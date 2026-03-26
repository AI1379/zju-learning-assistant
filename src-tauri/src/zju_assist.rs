use anyhow::{anyhow, Ok, Result};
use futures::{SinkExt, StreamExt};
use log::{debug, info};
use percent_encoding::percent_decode_str;
use regex::Regex;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{HeaderMap, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, Method, RequestBuilder, Response};
use reqwest::{Error, IntoUrl};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::cmp::min;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs::File, io::Write, path::Path};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::model::{LiveTranscriptLine, LiveTranscriptSessionStatus, Subject};
use crate::utils::{measure_latency, rsa_no_padding};

struct LiveTranscriptSession {
    course_id: i64,
    sub_id: i64,
    ws_url: String,
    started_at_ms: u64,
    lines: Arc<Mutex<Vec<LiveTranscriptLine>>>,
    task: JoinHandle<()>,
}

struct LiveTranscriptStore {
    sessions: HashMap<i64, LiveTranscriptSession>,
    auto_tasks: HashMap<i64, JoinHandle<()>>,
}

impl LiveTranscriptStore {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            auto_tasks: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct ZjuAssist {
    jar: Arc<Jar>,
    have_login: bool,
    username: String,
    password: String,
    proxy_first: bool,
    custom_proxy: Option<String>,
    live_transcript_store: Arc<Mutex<LiveTranscriptStore>>,
}

pub struct ZjuRequestBuilder {
    request_builder_first: RequestBuilder,
    request_builder_second: RequestBuilder,
}

#[derive(Debug, Deserialize)]
pub struct SubtitleContent {
    #[serde(rename = "BeginSec")]
    pub begin_sec: u64,
    #[serde(rename = "EndSec")]
    pub end_sec: u64,
    #[serde(rename = "Text")]
    pub text: String,
    #[serde(rename = "TransText")]
    pub trans_text: String,
}

#[derive(Debug, Deserialize)]
struct SubtitleItem {
    all_content: Vec<SubtitleContent>,
}

#[derive(Debug, Deserialize)]
struct SubtitleResponse {
    code: i64,
    list: Vec<SubtitleItem>,
}

#[derive(Debug, Deserialize)]
struct ZdbkSsoLoginUrlResponse {
    status: String,
    ssologinurl: Option<String>,
}

impl ZjuRequestBuilder {
    fn new<U: IntoUrl + Clone>(
        client: ZjuAssist,
        method: Method,
        url: U,
        proxy_first: bool,
    ) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );

        let mut client_default_builder = Client::builder()
            .cookie_provider(Arc::clone(&client.jar))
            .default_headers(headers.clone());

        if let Some(proxy_url) = client.custom_proxy.as_deref() {
            let proxy = reqwest::Proxy::all(proxy_url).unwrap();
            client_default_builder = client_default_builder.proxy(proxy);
        }

        let client_default = client_default_builder.build().unwrap();

        let client_no_proxy = Client::builder()
            .cookie_provider(Arc::clone(&client.jar))
            .default_headers(headers)
            .no_proxy()
            .build()
            .unwrap();
        if proxy_first {
            Self {
                request_builder_first: client_default.request(method.clone(), url.clone()),
                request_builder_second: client_no_proxy.request(method, url),
            }
        } else {
            Self {
                request_builder_first: client_no_proxy.request(method.clone(), url.clone()),
                request_builder_second: client_default.request(method, url),
            }
        }
    }

    pub fn headers(&mut self, headers: HeaderMap) -> &mut Self {
        self.request_builder_first = self
            .request_builder_first
            .try_clone()
            .unwrap()
            .headers(headers.clone());
        self.request_builder_second = self
            .request_builder_second
            .try_clone()
            .unwrap()
            .headers(headers.clone());
        self
    }

    pub fn form<T: Serialize + ?Sized>(&mut self, form: &T) -> &mut Self {
        self.request_builder_first = self.request_builder_first.try_clone().unwrap().form(form);
        self.request_builder_second = self.request_builder_second.try_clone().unwrap().form(form);
        self
    }

    pub async fn send(&self) -> Result<Response, Error> {
        // total 6 retries, 3 with proxy, 3 without proxy
        let mut res = self.request_builder_first.try_clone().unwrap().send().await;
        let mut retries = 5;
        let mut delay_time = 100;

        while res.is_err() && retries > 0 {
            retries -= 1;
            // wait for 200ms before retry
            tokio::time::sleep(Duration::from_millis(delay_time)).await;
            if retries % 2 == 0 {
                res = self
                    .request_builder_second
                    .try_clone()
                    .unwrap()
                    .send()
                    .await;
            } else {
                res = self.request_builder_first.try_clone().unwrap().send().await;
            }
            delay_time *= 2;
        }

        res
    }
}

impl ZjuAssist {
    pub fn new() -> Self {
        Self {
            jar: Arc::new(Jar::default()),
            have_login: false,
            username: "".to_string(),
            password: "".to_string(),
            proxy_first: true,
            custom_proxy: None,
            live_transcript_store: Arc::new(Mutex::new(LiveTranscriptStore::new())),
        }
    }

    pub fn set_custom_proxy(&mut self, proxy_url: Option<String>) -> Result<()> {
        if let Some(url) = proxy_url.as_deref() {
            reqwest::Proxy::all(url)
                .map_err(|err| anyhow!("Invalid proxy url '{}': {}", url, err))?;
        }
        self.custom_proxy = proxy_url;
        Ok(())
    }

    pub fn get_custom_proxy(&self) -> Option<String> {
        self.custom_proxy.clone()
    }

    pub fn request<U: IntoUrl + Clone>(&self, method: Method, url: U) -> ZjuRequestBuilder {
        ZjuRequestBuilder::new(self.clone(), method, url, self.proxy_first)
    }

    pub fn get<U: IntoUrl + Clone>(&self, url: U) -> ZjuRequestBuilder {
        info!("GET {}", url.as_str());
        self.request(Method::GET, url)
    }

    pub fn post<U: IntoUrl + Clone>(&self, url: U) -> ZjuRequestBuilder {
        info!("POST {}", url.as_str());
        self.request(Method::POST, url)
    }

    pub fn get_username(&self) -> String {
        self.username.clone()
    }

    pub async fn test_connection(&mut self) -> Result<()> {
        let headers = HeaderMap::new();
        let mut client_default_builder = Client::builder().default_headers(headers.clone());
        if let Some(proxy_url) = self.custom_proxy.as_deref() {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|err| anyhow!("Invalid proxy url '{}': {}", proxy_url, err))?;
            client_default_builder = client_default_builder.proxy(proxy);
        }
        let client_default = client_default_builder.build()?;
        let client_no_proxy = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .build()?;

        tokio::pin! {
            let latency_default = measure_latency(client_default, "https://zdbk.zju.edu.cn/");
            let latency_no_proxy = measure_latency(client_no_proxy, "https://zdbk.zju.edu.cn/");
        }

        let latency_default_result;
        let latency_no_proxy_result;

        tokio::select! {
            res = &mut latency_default => {
                if res.is_err() {
                    // if latency_default fail, wait for latency_no_proxy
                    latency_default_result = res;
                    latency_no_proxy_result = latency_no_proxy.await;
                } else {
                    // if latency_default success, no need to wait latency_no_proxy
                    info!("Latency default: {:?}", res);
                    self.proxy_first = true;
                    return Ok(());
                }
            },
            res = &mut latency_no_proxy => {
                if res.is_err() {
                    // if latency_no_proxy fail, wait for latency_default
                    latency_no_proxy_result = res;
                    latency_default_result = latency_default.await;
                } else {
                    // if latency_no_proxy success, no need to wait latency_default
                    info!("Latency no proxy: {:?}", res);
                    self.proxy_first = false;
                    return Ok(());
                }
            },
        }

        let latency_default = latency_default_result;
        let latency_no_proxy = latency_no_proxy_result;

        info!("Latency default: {:?}", latency_default);
        info!("Latency no proxy: {:?}", latency_no_proxy);

        if latency_default.is_err() && latency_no_proxy.is_err() {
            return Err(anyhow!("Connection failed"));
        }
        if latency_default.is_err() {
            self.proxy_first = false;
        } else if latency_no_proxy.is_err() {
            self.proxy_first = true;
        } else if latency_default.unwrap() < latency_no_proxy.unwrap() {
            self.proxy_first = true;
        } else {
            self.proxy_first = false;
        }

        info!("Proxy first: {}", self.proxy_first);
        Ok(())
    }

    pub async fn login(&mut self, username: &str, password: &str) -> Result<()> {
        if self.have_login {
            return Ok(());
        }

        let res = self
            .get("https://zjuam.zju.edu.cn/cas/login")
            .send()
            .await?;

        let mut text = res.text().await?;
        if !text.contains("统一身份认证平台") {
            self.logout();
            let res = self
                .get("https://zjuam.zju.edu.cn/cas/login")
                .send()
                .await?;
            text = res.text().await?;
            if !text.contains("统一身份认证平台") {
                return Err(anyhow!("Login failed"));
            }
        }
        let re = Regex::new(r#"<input type="hidden" name="execution" value="(.*?)" />"#).unwrap();
        let execution = re
            .captures(&text)
            .and_then(|cap| cap.get(1).map(|m| m.as_str()))
            .ok_or(anyhow!("Execution value not found"))?;
        let res = self
            .get("https://zjuam.zju.edu.cn/cas/v2/getPubKey")
            .send()
            .await?;

        let json: Value = res.json().await?;
        let modulus = json["modulus"]
            .as_str()
            .ok_or(anyhow!("Modulus not found"))?;
        let exponent = json["exponent"]
            .as_str()
            .ok_or(anyhow!("Exponent not found"))?;

        let rsapwd = rsa_no_padding(password, modulus, exponent);

        let data = [
            ("username", username),
            ("password", &rsapwd),
            ("execution", execution),
            ("_eventId", "submit"),
            ("authcode", ""),
        ];

        let res = self
            .post("https://zjuam.zju.edu.cn/cas/login")
            .form(&data)
            .send()
            .await?;

        if res.text().await?.contains("统一身份认证平台") {
            Err(anyhow!("Login failed: Wrong username or password"))
        } else {
            self.get("https://courses.zju.edu.cn/user/courses")
                .send()
                .await?;
            self.get("https://tgmedia.cmc.zju.edu.cn/index.php?r=auth/login&auType=cmc&tenant_code=112&forward=https%3A%2F%2Fclassroom.zju.edu.cn%2F")
                .send()
                .await?;
            self.post("https://zjuam.zju.edu.cn/cas/login?service=https://zdbk.zju.edu.cn/jwglxt/xtgl/login_ssologin.html")
                .send()
                .await?;
            self.have_login = true;
            self.username = username.to_string();
            self.password = password.to_string();

            Ok(())
        }
    }

    pub fn logout(&mut self) {
        self.jar = Arc::new(Jar::default());
        self.have_login = false;
        self.username = "".to_string();
        self.password = "".to_string();
    }

    pub async fn relogin(&mut self) -> Result<()> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let username = self.username.clone();
        let password = self.password.clone();
        let jar = Arc::clone(&self.jar);
        self.logout();
        let res = self.login(&username, &password).await;
        if res.is_err() {
            self.username = username;
            self.password = password;
            self.jar = jar;
            self.have_login = true;
        }
        res
    }

    pub fn is_login(&self) -> bool {
        self.have_login
    }

    // courses

    pub async fn get_courses(&self) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let mut courses = Vec::new();
        let res = self.get("https://courses.zju.edu.cn/api/my-courses?conditions=%7B%22status%22:%5B%22ongoing%22,%22notStarted%22%5D,%22keyword%22:%22%22,%22classify_type%22:%22recently_started%22,%22display_studio_list%22:false%7D&fields=id,name,course_code,department(id,name),grade(id,name),klass(id,name),course_type,cover,small_cover,start_date,end_date,is_started,is_closed,academic_year_id,semester_id,credit,compulsory,second_name,display_name,created_user(id,name),org(is_enterprise_or_organization),org_id,public_scope,audit_status,audit_remark,can_withdraw_course,imported_from,allow_clone,is_instructor,is_team_teaching,is_default_course_cover,instructors(id,name,email,avatar_small_url),course_attributes(teaching_class_name,is_during_publish_period,copy_status,tip,data),user_stick_course_record(id),classroom_schedule&page=1&page_size=100&showScorePassedStatus=false")
            .send()
            .await?;

        let json: Value = res.json().await?;
        courses.extend(json["courses"].as_array().unwrap().iter().cloned());
        if json["pages"].as_i64().unwrap() > 1 {
            for page in 2..=json["pages"].as_i64().unwrap() {
                let res = self.get(format!("https://courses.zju.edu.cn/api/my-courses?conditions=%7B%22status%22:%5B%22ongoing%22,%22notStarted%22%5D,%22keyword%22:%22%22,%22classify_type%22:%22recently_started%22,%22display_studio_list%22:false%7D&fields=id,name,course_code,department(id,name),grade(id,name),klass(id,name),course_type,cover,small_cover,start_date,end_date,is_started,is_closed,academic_year_id,semester_id,credit,compulsory,second_name,display_name,created_user(id,name),org(is_enterprise_or_organization),org_id,public_scope,audit_status,audit_remark,can_withdraw_course,imported_from,allow_clone,is_instructor,is_team_teaching,is_default_course_cover,instructors(id,name,email,avatar_small_url),course_attributes(teaching_class_name,is_during_publish_period,copy_status,tip,data),user_stick_course_record(id),classroom_schedule&page={}&page_size=100&showScorePassedStatus=false", page))
                    .send()
                    .await?;

                let json: Value = res.json().await?;
                courses.extend(json["courses"].as_array().unwrap().iter().cloned());
            }
        }
        Ok(courses)
    }

    pub async fn get_activities_uploads(&self, course_id: i64) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let mut uploads = Vec::new();
        let res = self
            .get(format!(
                "https://courses.zju.edu.cn/api/courses/{}/activities",
                course_id
            ))
            .send()
            .await?;
        let json: Value = res.json().await?;
        let activities = json["activities"].as_array().unwrap();
        for activity in activities {
            if activity["uploads"].is_array() {
                uploads.extend(activity["uploads"].as_array().unwrap().iter().cloned());
            }
        }
        Ok(uploads)
    }

    pub async fn get_homework_uploads(&self, course_id: i64) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let mut uploads = Vec::new();
        let res = self.get(format!("https://courses.zju.edu.cn/api/courses/{}/homework-activities?conditions=%7B%22itemsSortBy%22:%7B%22predicate%22:%22module%22,%22reverse%22:false%7D%7D&page=1&page_size=20&reloadPage=false", course_id))
            .send()
            .await?;
        let json: Value = res.json().await?;
        let homeworks = json["homework_activities"].as_array().unwrap();
        for homework in homeworks {
            if homework["uploads"].is_array() {
                uploads.extend(homework["uploads"].as_array().unwrap().iter().cloned());
            }
        }
        if json["pages"].as_i64().unwrap() > 1 {
            for page in 2..=json["pages"].as_i64().unwrap() {
                let res = self.get(format!("https://courses.zju.edu.cn/api/courses/{}/homework-activities?conditions=%7B%22itemsSortBy%22:%7B%22predicate%22:%22module%22,%22reverse%22:false%7D%7D&page={}&page_size=20&reloadPage=false", course_id, page))
                    .send()
                    .await?;
                let json: Value = res.json().await?;
                let homeworks = json["homework_activities"].as_array().unwrap();
                for homework in homeworks {
                    if homework["uploads"].is_array() {
                        uploads.extend(homework["uploads"].as_array().unwrap().iter().cloned());
                    }
                }
            }
        }
        Ok(uploads)
    }

    pub async fn download_file(
        &self,
        id: i64,
        reference_id: i64,
        name: &str,
        path: &str,
    ) -> Result<()> {
        let res = self
            .get(format!(
                "https://courses.zju.edu.cn/api/uploads/reference/{}/blob",
                reference_id
            ))
            .send()
            .await?;
        let mut filename = name.to_string();
        // if the upload is not allowed to download, then get the preview url
        let res = match res.status().is_success() {
            true => res,
            false => {
                self.get(format!(
                    "https://courses.zju.edu.cn/api/uploads/{}/blob",
                    id
                ))
                .send()
                .await?
            }
        };
        std::fs::create_dir_all(Path::new(path))?;
        let mut file = File::create(Path::new(path).join(filename))?;
        let content = res.bytes().await?;
        file.write_all(&content)?;
        Ok(())
    }

    pub async fn get_uploads_response(&self, id: i64, reference_id: i64) -> Result<Response> {
        const MAX_RETRIES: usize = 5;
        let mut retries = 0;
        let mut delay_time = 100;

        while retries < MAX_RETRIES {
            let res = self
                .get(format!(
                    "https://courses.zju.edu.cn/api/uploads/reference/{}/blob",
                    reference_id
                ))
                .send()
                .await?;
            // if the upload is not allowed to download, then get the preview url
            let res = match res.status().is_success() {
                true => res,
                false => {
                    self.get(format!(
                        "https://courses.zju.edu.cn/api/uploads/{}/blob",
                        id
                    ))
                    .send()
                    .await?
                }
            };
            if res.status().is_success() {
                return Ok(res);
            }
            tokio::time::sleep(Duration::from_millis(delay_time)).await;
            retries += 1;
            delay_time *= 2;
        }
        Err(anyhow!("Failed to get upload response"))
    }

    pub async fn get_academic_year_list(&self) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let res = self
            .get("https://courses.zju.edu.cn/api/my-academic-years?fields=id,name,sort,is_active")
            .send()
            .await?;
        let json: Value = res.json().await?;
        Ok(json["academic_years"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .collect())
    }

    pub async fn get_semester_list(&self) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let res = self
            .get("https://courses.zju.edu.cn/api/my-semesters?")
            .send()
            .await?;
        let json: Value = res.json().await?;
        Ok(json["semesters"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .collect())
    }

    pub async fn get_todo_list(&self) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let res = self
            .get("https://courses.zju.edu.cn/api/todos?no-intercept=true")
            .send()
            .await?;
        let json: Value = res.json().await?;
        Ok(json["todo_list"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .collect())
    }

    // classroom

    pub fn get_token(&self) -> Result<String> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        if let Some(cookies) = self
            .jar
            .cookies(&url::Url::parse("https://classroom.zju.edu.cn")?)
        {
            let cookie_str = percent_decode_str(cookies.to_str().unwrap())
                .decode_utf8_lossy()
                .to_string();
            let re = Regex::new(r#"\{i:\d+;s:\d+:"_token";i:\d+;s:\d+:"(.+?)";\}"#).unwrap();
            let token = re
                .captures(&cookie_str)
                .and_then(|cap| cap.get(1).map(|m| m.as_str()))
                .ok_or(anyhow!("Token not found, try log in again"))?;
            Ok(token.to_string())
        } else {
            Err(anyhow!("Token not found, try log in again"))
        }
    }

    pub async fn keep_classroom_alive(&mut self) -> Result<()> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let token = self.get_token();
        if let Err(_) = token {
            self.relogin().await?;
        }
        Ok(())
    }

    pub async fn get_month_subs(&self, month: &str) -> Result<Vec<Subject>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let token = self.get_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );
        headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());

        let mut subs = Vec::new();

        let res = self.get(format!("https://classroom.zju.edu.cn/courseapi/v2/course-live/get-my-course-month?month={}", month))
            .headers(headers.clone())
            .send()
            .await?;
        let json: Value = res.json().await?;
        let list = json["list"].as_array().unwrap();
        for day in list {
            let courses = day["course"].as_array().unwrap();
            for course in courses {
                let course_id = course["id"].as_str().unwrap().parse::<i64>().unwrap();
                let course_name = course["title"].as_str().unwrap().replace("/", "_");
                let sub_id = course["sub_id"].as_str().unwrap().parse::<i64>().unwrap();
                let sub_name = course["sub_title"].as_str().unwrap().replace("/", "_");
                let lecturer_name = course["realname"].as_str().unwrap().to_string();
                let start_at = course["start_at"]
                    .as_i64()
                    .or_else(|| course["start_at"].as_str().and_then(|s| s.parse::<i64>().ok()));
                let room = course["room"]
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| course["room"].as_i64().map(|v| v.to_string()));
                let tenant_code = course["tenant_code"]
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| course["tenant_code"].as_i64().map(|v| v.to_string()));
                let sub_public = course["sub_public"]
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| course["sub_public"].as_i64().map(|v| v.to_string()));
                subs.push(Subject {
                    course_id,
                    course_name: course_name.clone(),
                    sub_id,
                    sub_name,
                    lecturer_name,
                    path: "".to_string(), // path will be set when downloading
                    ppt_image_urls: Vec::new(),
                    start_at,
                    room,
                    tenant_code,
                    sub_public,
                });
            }
        }
        Ok(subs)
    }

    pub async fn get_range_subs(
        &self,
        start: &str, // format: 2021-05-01
        end: &str,
    ) -> Result<Vec<Subject>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let token = self.get_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );
        headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());

        let mut subs = Vec::new();

        // enumerate all days
        let start = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap();
        let end = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap();
        let mut date = start;
        while date <= end {
            let res = self.get(format!("https://classroom.zju.edu.cn/courseapi/v2/course-live/get-my-course-day?day={}", date.format("%Y-%m-%d")))
                .headers(headers.clone())
                .send()
                .await?;
            let json: Value = res.json().await?;
            if let Some(list) = json["list"].as_object() {
                for data in list.values() {
                    let courses = data["course"].as_array().unwrap();
                    for course in courses {
                        let course_id = course["id"].as_str().unwrap().parse::<i64>().unwrap();
                        let course_name = course["title"].as_str().unwrap().replace("/", "_");
                        let sub_id = course["sub_id"].as_str().unwrap().parse::<i64>().unwrap();
                        let sub_name = course["sub_title"].as_str().unwrap().replace("/", "_");
                        let lecturer_name = course["realname"].as_str().unwrap().to_string();
                        let start_at = course["start_at"].as_i64().or_else(|| {
                            course["start_at"].as_str().and_then(|s| s.parse::<i64>().ok())
                        });
                        let room = course["room"]
                            .as_str()
                            .map(|s| s.to_string())
                            .or_else(|| course["room"].as_i64().map(|v| v.to_string()));
                        let tenant_code = course["tenant_code"]
                            .as_str()
                            .map(|s| s.to_string())
                            .or_else(|| course["tenant_code"].as_i64().map(|v| v.to_string()));
                        let sub_public = course["sub_public"]
                            .as_str()
                            .map(|s| s.to_string())
                            .or_else(|| course["sub_public"].as_i64().map(|v| v.to_string()));
                        subs.push(Subject {
                            course_id,
                            course_name: course_name.clone(),
                            sub_id,
                            sub_name,
                            lecturer_name,
                            path: "".to_string(), // path will be set when downloading
                            ppt_image_urls: Vec::new(),
                            start_at,
                            room,
                            tenant_code,
                            sub_public,
                        });
                    }
                }
            }

            date = date + chrono::Duration::try_days(1).unwrap();
        }

        Ok(subs)
    }

    pub async fn search_courses(
        &self,
        course_name: &str,
        teacher_name: &str,
    ) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let token = self.get_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );
        headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());

        let res = self
            .get("https://classroom.zju.edu.cn/userapi/v1/infosimple")
            .headers(headers.clone())
            .send()
            .await?;
        let json: Value = res.json().await?;
        let account = json["params"]["account"].as_str().unwrap();
        let user_id = json["params"]["id"].as_i64().unwrap();
        let random: f64 = rand::random();

        let res = self.get(format!("https://classroom.zju.edu.cn/pptnote/v1/searchlist?tenant_id=112&user_id={}&user_name={}&page=1&per_page=16&title={}&realname={}&trans=&tenant_code=112&randomKey={}", user_id, account, course_name, teacher_name, random))
            .headers(headers.clone())
            .send()
            .await?;

        let mut courses = Vec::new();
        let json: Value = res.json().await?;

        // if code is not 0, then there is an error
        if json["code"].as_i64().unwrap() != 0 {
            let msg = json["msg"].as_str().unwrap();
            return Err(anyhow!(msg.to_string()));
        }

        courses.extend(json["total"]["list"].as_array().unwrap().iter().cloned());
        let mut page = 1;
        let total_course = json["total"]["total"].as_i64().unwrap();
        while courses.len() < total_course as usize {
            page += 1;
            let random: f64 = rand::random();
            let res = self.get(format!("https://classroom.zju.edu.cn/pptnote/v1/searchlist?tenant_id=112&user_id={}&user_name={}&page={}&per_page=16&title={}&realname={}&trans=&tenant_code=112&randomKey={}", user_id, account, page, course_name, teacher_name, random))
                .headers(headers.clone())
                .send()
                .await?;
            let json: Value = res.json().await?;
            courses.extend(json["total"]["list"].as_array().unwrap().iter().cloned());
        }

        Ok(courses)
    }

    pub async fn get_course_subs(&self, course_id: i64) -> Result<Vec<Subject>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }
        let token = self.get_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );
        headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());

        let res = self
            .get("https://classroom.zju.edu.cn/userapi/v1/infosimple")
            .headers(headers.clone())
            .send()
            .await?;
        let json: Value = res.json().await?;
        let account = json["params"]["account"].as_str().unwrap();

        let res = self.get(format!("https://yjapi.cmc.zju.edu.cn/courseapi/v3/multi-search/get-course-detail?course_id={}&student={}", course_id, account))
            .headers(headers.clone())
            .send()
            .await?;
        let json: Value = res.json().await?;
        let data = json["data"].as_object().unwrap();
        let course_name = data["title"].as_str().unwrap().replace("/", "_");
        let sub_list = data["sub_list"].as_object().unwrap();
        let mut subs = Vec::new();
        for (_, year_data) in sub_list {
            for (_, month_data) in year_data.as_object().unwrap() {
                for (_, week_data) in month_data.as_object().unwrap() {
                    for sub in week_data.as_array().unwrap() {
                        let sub_id = sub["id"].as_str().unwrap().parse::<i64>().unwrap();
                        let sub_name = sub["sub_title"].as_str().unwrap().replace("/", "_");
                        let lecturer_name = sub["lecturer_name"].as_str().unwrap().to_string();
                        let start_at = sub["start_at"]
                            .as_i64()
                            .or_else(|| sub["start_at"].as_str().and_then(|s| s.parse::<i64>().ok()));
                        let room = sub["room"]
                            .as_str()
                            .map(|s| s.to_string())
                            .or_else(|| sub["room"].as_i64().map(|v| v.to_string()));
                        let tenant_code = sub["tenant_code"]
                            .as_str()
                            .map(|s| s.to_string())
                            .or_else(|| sub["tenant_code"].as_i64().map(|v| v.to_string()));
                        let sub_public = sub["sub_public"]
                            .as_str()
                            .map(|s| s.to_string())
                            .or_else(|| sub["sub_public"].as_i64().map(|v| v.to_string()));
                        subs.push(Subject {
                            course_id,
                            course_name: course_name.clone(),
                            sub_id,
                            sub_name,
                            lecturer_name,
                            path: "".to_string(), // path will be set when downloading
                            ppt_image_urls: Vec::new(),
                            start_at,
                            room,
                            tenant_code,
                            sub_public,
                        });
                    }
                }
            }
        }
        Ok(subs)
    }

    fn get_auth_play_url(url: &str, id: &str, tenant_id: &str, phone: &str) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let revers_phone = phone.chars().rev().collect::<String>();
        // t= id-timestamp-md5(uri+id+tenant_id+reverse(phone)+timestamp)
        let resource_url = Url::parse(url).unwrap().path().to_string();
        let key = format!(
            "{}-{}-{:x}",
            id,
            timestamp,
            md5::compute(format!(
                "{}{}{}{}{}",
                resource_url, id, tenant_id, revers_phone, timestamp
            ))
        );

        if url.contains("?") {
            format!("{}&t={}", url, key)
        } else {
            format!("{}?t={}", url, key)
        }
    }

    pub async fn get_playback_response(&self, course_id: i64, sub_id: i64) -> Result<Response> {
        let res = self
            .get(format!(
                "https://classroom.zju.edu.cn/courseapi/v3/portal-home-setting/get-sub-info?course_id={}&sub_id={}",
                course_id, sub_id
            ))
            .send()
            .await?;
        let json: Value = res.json().await?;
        let url = json["data"]["content"]["save_playback"]["contents"]
            .as_str()
            .unwrap();

        let res = self
            .get("https://classroom.zju.edu.cn/userapi/v1/infosimple")
            .send()
            .await?;
        let json: Value = res.json().await?;
        println!("{}", json["params"]);
        let id = json["params"]["id"].as_i64().unwrap().to_string();
        let tenant_id = json["params"]["tenant_id"].as_i64().unwrap().to_string();
        let phone = json["params"]["phone"].as_str().unwrap();

        let url = Self::get_auth_play_url(url, &id, &tenant_id, phone);
        let res = self.get(url).send().await?;

        Ok(res)
    }

    pub async fn get_ppt_urls(&self, course_id: i64, sub_id: i64) -> Result<Vec<String>> {
        let mut urls = Vec::new();
        let res = self.get(format!("https://classroom.zju.edu.cn/pptnote/v1/schedule/search-ppt?course_id={}&sub_id={}&page=1&per_page=100", course_id, sub_id)).send()
            .await?;
        let json: Value = res.json().await?;
        let ppt_list = json["list"].as_array().unwrap();
        let mut page = 1;
        let total_ppt = json["total"].as_i64().unwrap();
        for ppt_content in ppt_list {
            let content: Value =
                serde_json::from_str(ppt_content["content"].as_str().unwrap()).unwrap();
            let url = content["pptimgurl"].as_str().unwrap();
            urls.push(url.to_string());
        }
        let mut retries = 5;
        while urls.len() < total_ppt as usize {
            page += 1;
            let res = self.get(format!("https://classroom.zju.edu.cn/pptnote/v1/schedule/search-ppt?course_id={}&sub_id={}&page={}&per_page=100", course_id, sub_id, page)).send()
                .await?;
            let json: Value = res.json().await?;
            let should_have = min(100, total_ppt as usize - urls.len());
            let ppt_list = json["list"].as_array().unwrap();
            if ppt_list.len() != should_have {
                page -= 1;
                retries -= 1;
                if retries == 0 {
                    Err(anyhow!(
                        "Get ppt urls failed for course_id: {}, sub_id: {}, please retry later.",
                        course_id,
                        sub_id
                    ))?;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            for ppt_content in ppt_list {
                let content: Value =
                    serde_json::from_str(ppt_content["content"].as_str().unwrap()).unwrap();
                let url = content["pptimgurl"].as_str().unwrap();
                urls.push(url.to_string());
            }
        }
        Ok(urls)
    }

    pub async fn download_ppt_image(&self, url: &str, path: &str) -> Result<()> {
        const MAX_RETRIES: usize = 5;
        let mut retries = 0;
        let mut delay_time = 100;

        let file_path = match Path::new(path).extension() {
            Some(_) => Path::new(path).to_path_buf(),
            None => Path::new(path).join(url.split("/").last().unwrap()),
        };

        while retries < MAX_RETRIES {
            let res = self.get(url).send().await?;
            let content = res.bytes().await?;
            if content.is_empty() || image::guess_format(&content).is_err() {
                retries += 1;
                continue;
            }

            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = File::create(file_path.clone())?;
            file.write_all(&content)?;

            let metadata = file.metadata()?;
            if metadata.len() > 0 {
                return Ok(());
            } else {
                tokio::time::sleep(Duration::from_millis(delay_time)).await;
                retries += 1;
                delay_time *= 2;
            }
        }

        // clean up
        if file_path.exists() {
            std::fs::remove_file(file_path)?;
        }

        Err(anyhow!("Failed to download file after several attempts"))
    }

    // zdbk

    async fn ensure_zdbk_session(&mut self) -> Result<()> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }

        let service = "https://zdbk.zju.edu.cn/jwglxt/xtgl/login_slogin.html";
        let encoded_service: String =
            url::form_urlencoded::byte_serialize(service.as_bytes()).collect();
        let cas_service_url = format!(
            "https://zjuam.zju.edu.cn/cas/login?service={}",
            encoded_service
        );

        self.get(cas_service_url).send().await?;
        self.get(service).send().await?;

        let sso_meta_url = "https://zdbk.zju.edu.cn/jwglxt/xtgl/login_cxSsoLoginUrl.html";
        let sso_meta_resp = self.post(sso_meta_url).send().await?;
        let sso_meta_text = sso_meta_resp.text().await?;
        let sso_meta: ZdbkSsoLoginUrlResponse = serde_json::from_str(&sso_meta_text)
            .map_err(|e| anyhow!("Parse zdbk SSO login url failed: {}", e))?;

        if sso_meta.status == "success" {
            if let Some(sso_login_url) = sso_meta.ssologinurl {
                self.get(sso_login_url).send().await?;
                return Ok(());
            } else {
                return Err(anyhow!(
                    "zdbk SSO login failed: ssologinurl missing in successful response"
                ));
            }
        }

        Err(anyhow!(
            "zdbk SSO login failed: status is '{}'",
            sso_meta.status
        ))
    }

    pub async fn check_evaluation_done(&mut self) -> Result<bool> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }

        let res = self
            .post(format!(
                "https://zdbk.zju.edu.cn/jwglxt/xtgl/index_cxMyCosJxpj.html?gnmkdm=N5083&su={}",
                self.username
            ))
            .send()
            .await?;
        let text = res.text().await?;
        let mut json = serde_json::from_str(&text);

        if json.is_err() {
            self.relogin().await?;

            let res = self
                .post(format!(
                    "https://zdbk.zju.edu.cn/jwglxt/xtgl/index_cxMyCosJxpj.html?gnmkdm=N5083&su={}",
                    self.username
                ))
                .send()
                .await?;
            let text = res.text().await?;
            json = serde_json::from_str(&text);
            if json.is_err() {
                return Err(anyhow!("Check evaluation failed"));
            }
        }

        let json: Value = json.unwrap();
        let result = json["result"].as_str().unwrap();

        Ok(result == "1")
    }

    pub async fn get_score(&mut self) -> Result<Vec<Value>> {
        if let Err(err) = self.ensure_zdbk_session().await {
            info!("ensure_zdbk_session before get_score failed: {}", err);
            return Err(anyhow!("Get score failed: ensure session failed"));
        }

        if let Err(err) = self.probe_score().await {
            info!("probe_score before get_score failed: {}", err);
            self.relogin().await?;
            self.ensure_zdbk_session().await?;
        }

        let data = [
            ("xn", ""),
            ("xq", ""),
            ("zscjl", ""),
            ("zscjr", ""),
            ("_search", "false"),
            (
                "nd",
                &SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
                    .to_string(),
            ),
            ("queryModel.showCount", "5000"),
            ("queryModel.currentPage", "1"),
            ("queryModel.sortName", "xkkh"),
            ("queryModel.sortOrder", "asc"),
            ("time", "1"),
        ];

        let score_url = format!(
            "https://zdbk.zju.edu.cn/jwglxt/cxdy/xscjcx_cxXscjIndex.html?doType=query&gnmkdm=N508301&su={}",
            self.username
        );

        let res = self.post(&score_url).form(&data).send().await?;
        let first_status = res.status();
        let first_url = res.url().to_string();
        let text = res.text().await?;

        let json = serde_json::from_str(&text);
        if json.is_err() {
            debug!(
                "get_score first response not json: status={} url={} text={}",
                first_status, first_url, text
            );
            return Err(anyhow!("Get score failed"));
        }
        let json: Value = json.unwrap();
        let score = json["items"].as_array().unwrap();
        return Ok(score.iter().cloned().collect());
    }

    async fn probe_score(&self) -> Result<()> {
        let probe_data = [
            ("xn", ""),
            ("xq", ""),
            ("zscjl", ""),
            ("zscjr", ""),
            ("_search", "false"),
            ("nd", "0"),
            ("queryModel.showCount", "1"),
            ("queryModel.currentPage", "1"),
            ("queryModel.sortName", "xkkh"),
            ("queryModel.sortOrder", "asc"),
            ("time", "1"),
        ];

        let score_probe_url = format!(
            "https://zdbk.zju.edu.cn/jwglxt/cxdy/xscjcx_cxXscjIndex.html?doType=query&gnmkdm=N508301&su={}",
            self.username
        );

        let res = self.post(score_probe_url).form(&probe_data).send().await?;
        let text = res.text().await?;
        if serde_json::from_str::<Value>(&text).is_ok() {
            return Ok(());
        }
        if text.contains("统一身份认证平台")
            || text.contains("/cas/login")
            || text.contains("login_slogin")
        {
            return Err(anyhow!("Probe score failed: redirected to login page"));
        }

        Err(anyhow!("Probe score failed"))
    }

    async fn get_trans_socket_url(&self, course_id: i64, sub_id: i64) -> Result<String> {
        let token = self.get_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );
        headers.insert(AUTHORIZATION, format!("Bearer {}", token).parse().unwrap());

        let res = self
            .get(format!(
                "https://yjapi.cmc.zju.edu.cn/courseapi/v2/course/catalogue?course_id={}",
                course_id
            ))
            .headers(headers)
            .send()
            .await?;
        let json: Value = res.json().await?;

        let mut candidate_subs: Vec<Value> = Vec::new();

        if let Some(catalogue) = json["data"]["course"]["catalogue"].as_array() {
            for node in catalogue {
                if let Some(subs) = node.get("sub").and_then(|v| v.as_array()) {
                    candidate_subs.extend(subs.iter().cloned());
                }
            }
        }

        if let Some(flat_list) = json["result"]["data"].as_array() {
            candidate_subs.extend(flat_list.iter().cloned());
        }

        for sub in candidate_subs {
            let candidate = sub
                .get("sub_id")
                .or_else(|| sub.get("id"))
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .or_else(|| v.as_i64())
                });
            if candidate != Some(sub_id) {
                continue;
            }

            let content_value = sub.get("content").cloned().unwrap_or(Value::Null);
            let content_json = if content_value.is_object() {
                content_value
            } else if let Some(content_str) = content_value.as_str() {
                serde_json::from_str(content_str)
                    .map_err(|err| anyhow!("parse sub content failed: {}", err))?
            } else {
                Value::Null
            };

            if let Some(url) = content_json
                .get("trans_socket_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                return Ok(url);
            }
        }

        Err(anyhow!("trans_socket_url not found for sub_id {}", sub_id))
    }

    fn normalize_live_ws_url(trans_socket_url: &str) -> String {
        let mut ws_url = trans_socket_url.trim().to_string();
        if ws_url.starts_with("https://") {
            ws_url = ws_url.replacen("https://", "wss://", 1);
        } else if ws_url.starts_with("http://") {
            ws_url = ws_url.replacen("http://", "wss://", 1);
        } else if ws_url.starts_with("ws://") {
            ws_url = ws_url.replacen("ws://", "wss://", 1);
        }

        if !ws_url.ends_with("/glue/ws") {
            ws_url = ws_url.trim_end_matches('/').to_string();
            ws_url.push_str("/glue/ws");
        }
        ws_url
    }

    fn parse_live_line(value: &Value) -> Option<LiveTranscriptLine> {
        let source_text = value
            .get("sourcetext")
            .or_else(|| value.get("source_text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let trans_text = value
            .get("transtext")
            .or_else(|| value.get("trans_text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if source_text.is_empty() && trans_text.is_empty() {
            return None;
        }

        let text_begin_time = value.get("text_begin_time").and_then(|v| v.as_i64());
        let text_end_time = value.get("text_end_time").and_then(|v| v.as_i64());
        let end_time = value.get("end_time").and_then(|v| v.as_i64());

        let received_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_millis(0))
            .as_millis() as u64;

        Some(LiveTranscriptLine {
            source_text,
            trans_text,
            text_begin_time,
            text_end_time,
            end_time,
            received_at_ms,
        })
    }

    async fn parse_live_ws_message(
        lines: Arc<Mutex<Vec<LiveTranscriptLine>>>,
        dedup_keys: Arc<Mutex<HashMap<String, ()>>>,
        text: &str,
    ) {
        let payload = if let Some(rest) = text.strip_prefix("cd1&m") {
            rest
        } else {
            return;
        };

        let json: Value = match serde_json::from_str(payload) {
            std::result::Result::Ok(value) => value,
            std::result::Result::Err(_) => return,
        };

        let mut candidates = Vec::new();
        if json.is_object() {
            candidates.push(json.clone());
        }
        if let Some(arr) = json.get("list").and_then(|v| v.as_array()) {
            candidates.extend(arr.iter().cloned());
        }

        if candidates.is_empty() {
            return;
        }

        let mut lines_lock = lines.lock().await;
        let mut dedup_lock = dedup_keys.lock().await;
        for candidate in candidates {
            let Some(line) = Self::parse_live_line(&candidate) else {
                continue;
            };
            let dedup_key = format!(
                "{:?}|{:?}|{}|{}",
                line.text_begin_time, line.text_end_time, line.source_text, line.trans_text
            );
            if dedup_lock.contains_key(&dedup_key) {
                continue;
            }
            dedup_lock.insert(dedup_key, ());
            lines_lock.push(line);
        }
    }

    pub async fn start_live_transcript_capture(
        &self,
        course_id: i64,
        sub_id: i64,
    ) -> Result<LiveTranscriptSessionStatus> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }

        let existing_lines = {
            let mut store = self.live_transcript_store.lock().await;
            if let Some(existing) = store.sessions.remove(&sub_id) {
                existing.task.abort();
                Some(existing.lines)
            } else {
                None
            }
        };

        {
            let mut store = self.live_transcript_store.lock().await;
            if let Some(task) = store.auto_tasks.remove(&sub_id) {
                task.abort();
            }
        }

        let trans_socket_url = self.get_trans_socket_url(course_id, sub_id).await?;
        let ws_url = Self::normalize_live_ws_url(&trans_socket_url);

        let lines = existing_lines.unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())));
        let dedup_keys = Arc::new(Mutex::new(HashMap::new()));

        {
            let lines_snapshot = lines.lock().await.clone();
            let mut dedup_lock = dedup_keys.lock().await;
            for line in &lines_snapshot {
                let dedup_key = format!(
                    "{:?}|{:?}|{}|{}",
                    line.text_begin_time, line.text_end_time, line.source_text, line.trans_text
                );
                dedup_lock.insert(dedup_key, ());
            }
        }

        let ws_url_clone = ws_url.clone();
        let mut ws_request = ws_url_clone
            .clone()
            .into_client_request()
            .map_err(|err| anyhow!("invalid ws url {}: {}", ws_url_clone, err))?;
        ws_request.headers_mut().insert(
            "Origin",
            "https://classroom.zju.edu.cn".parse().unwrap(),
        );
        ws_request.headers_mut().insert(
            USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0"
                .parse()
                .unwrap(),
        );

        let (stream, _) = connect_async(ws_request).await.map_err(|err| {
            anyhow!(
                "live transcript ws connect failed: {} ({})",
                ws_url_clone,
                err
            )
        })?;

        let lines_clone = Arc::clone(&lines);
        let dedup_clone = Arc::clone(&dedup_keys);
        let task = tokio::spawn(async move {
            let (mut write, mut read) = stream.split();
            if write
                .send(Message::Text("in{\"version\":\"1.9.1\"}".to_string().into()))
                .await
                .is_err()
            {
                info!("live transcript ws init failed: {}", ws_url_clone);
                return;
            }

            let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
            let _ = heartbeat.tick().await;

            loop {
                tokio::select! {
                    maybe_msg = read.next() => {
                        let Some(msg) = maybe_msg else {
                            break;
                        };
                        let msg = match msg {
                            std::result::Result::Ok(value) => value,
                            std::result::Result::Err(_) => break,
                        };
                        match msg {
                            Message::Text(text) => {
                                Self::parse_live_ws_message(
                                    Arc::clone(&lines_clone),
                                    Arc::clone(&dedup_clone),
                                    &text,
                                )
                                .await;
                            }
                            Message::Binary(_) => {}
                            Message::Close(_) => break,
                            Message::Ping(_) => {}
                            Message::Pong(_) => {}
                            Message::Frame(_) => {}
                        }
                    }
                    _ = heartbeat.tick() => {
                        if write.send(Message::Text("po".to_string().into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_millis(0))
            .as_millis() as u64;

        let session = LiveTranscriptSession {
            course_id,
            sub_id,
            ws_url: ws_url.clone(),
            started_at_ms,
            lines,
            task,
        };

        let mut store = self.live_transcript_store.lock().await;
        store.sessions.insert(sub_id, session);

        Ok(LiveTranscriptSessionStatus {
            course_id,
            sub_id,
            ws_url,
            started_at_ms,
            line_count: 0,
            is_running: true,
        })
    }

    pub async fn schedule_live_transcript_capture_at(
        &self,
        course_id: i64,
        sub_id: i64,
        start_at: i64,
    ) -> Result<()> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs() as i64;

        if start_at <= now + 2 {
            let _ = self.start_live_transcript_capture(course_id, sub_id).await;
            return Ok(());
        }

        {
            let store = self.live_transcript_store.lock().await;
            if store.sessions.contains_key(&sub_id) || store.auto_tasks.contains_key(&sub_id) {
                return Ok(());
            }
        }

        let delay_secs = (start_at - now) as u64;
        let assist = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            let _ = assist.start_live_transcript_capture(course_id, sub_id).await;
            let mut store = assist.live_transcript_store.lock().await;
            store.auto_tasks.remove(&sub_id);
        });

        let mut store = self.live_transcript_store.lock().await;
        store.auto_tasks.insert(sub_id, task);
        Ok(())
    }

    pub async fn cancel_scheduled_live_transcript_capture(&self, sub_id: i64) -> Result<()> {
        let mut store = self.live_transcript_store.lock().await;
        let Some(task) = store.auto_tasks.remove(&sub_id) else {
            return Err(anyhow!("Scheduled task not found for sub_id {}", sub_id));
        };
        task.abort();
        Ok(())
    }

    pub async fn stop_live_transcript_capture(&self, sub_id: i64) -> Result<()> {
        let mut store = self.live_transcript_store.lock().await;
        let Some(session) = store.sessions.get_mut(&sub_id) else {
            return Err(anyhow!("Live transcript session not found for sub_id {}", sub_id));
        };
        session.task.abort();
        Ok(())
    }

    pub async fn clear_live_transcript_session(&self, sub_id: i64) -> Result<()> {
        let mut store = self.live_transcript_store.lock().await;
        if let Some(task) = store.auto_tasks.remove(&sub_id) {
            task.abort();
        }
        let Some(session) = store.sessions.remove(&sub_id) else {
            return Err(anyhow!("Live transcript session not found for sub_id {}", sub_id));
        };
        session.task.abort();
        Ok(())
    }

    pub async fn get_live_transcript_lines(&self, sub_id: i64) -> Result<Vec<LiveTranscriptLine>> {
        let lines_arc = {
            let store = self.live_transcript_store.lock().await;
            let session = store
                .sessions
                .get(&sub_id)
                .ok_or(anyhow!("Live transcript session not found for sub_id {}", sub_id))?;
            Arc::clone(&session.lines)
        };
        let lines = lines_arc.lock().await;
        Ok(lines.clone())
    }

    pub async fn get_live_transcript_sessions(&self) -> Vec<LiveTranscriptSessionStatus> {
        let snapshots = {
            let store = self.live_transcript_store.lock().await;
            let mut data = Vec::new();
            for session in store.sessions.values() {
                data.push((
                    session.course_id,
                    session.sub_id,
                    session.ws_url.clone(),
                    session.started_at_ms,
                    Arc::clone(&session.lines),
                    session.task.is_finished(),
                ));
            }
            data
        };

        let mut result = Vec::new();
        for (course_id, sub_id, ws_url, started_at_ms, lines_arc, is_finished) in snapshots {
            let line_count = lines_arc.lock().await.len();
            result.push(LiveTranscriptSessionStatus {
                course_id,
                sub_id,
                ws_url,
                started_at_ms,
                line_count,
                is_running: !is_finished,
            });
        }
        result
    }

    pub async fn export_live_transcript_text(
        &self,
        sub_id: i64,
        include_original: bool,
        with_timestamps: bool,
    ) -> Result<String> {
        let lines = self.get_live_transcript_lines(sub_id).await?;
        let mut chunks = Vec::new();
        for line in lines {
            let mut row = String::new();
            if with_timestamps {
                let begin = line
                    .text_begin_time
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let end = line
                    .text_end_time
                    .or(line.end_time)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string());
                row.push_str(&format!("[{}-{}] ", begin, end));
            }
            if include_original && !line.source_text.is_empty() {
                row.push_str(&line.source_text);
                row.push('\n');
            }
            if !line.trans_text.is_empty() {
                row.push_str(&line.trans_text);
            }
            if row.trim().is_empty() {
                continue;
            }
            chunks.push(row);
        }
        Ok(chunks.join("\n"))
    }

    pub async fn backfill_live_transcript_from_history(&self, sub_id: i64) -> Result<usize> {
        let subtitle = self.get_subtitle(sub_id).await?;
        let session_arcs = {
            let store = self.live_transcript_store.lock().await;
            let session = store
                .sessions
                .get(&sub_id)
                .ok_or(anyhow!("Live transcript session not found for sub_id {}", sub_id))?;
            (Arc::clone(&session.lines),)
        };
        let (lines_arc,) = session_arcs;

        let mut lines_lock = lines_arc.lock().await;
        let mut existing = HashMap::new();
        for line in &*lines_lock {
            let key = format!(
                "{:?}|{:?}|{}|{}",
                line.text_begin_time, line.text_end_time, line.source_text, line.trans_text
            );
            existing.insert(key, ());
        }

        let mut inserted = 0usize;
        for item in subtitle {
            let line = LiveTranscriptLine {
                source_text: item.text.trim().to_string(),
                trans_text: item.trans_text.trim().to_string(),
                text_begin_time: Some(item.begin_sec as i64),
                text_end_time: Some(item.end_sec as i64),
                end_time: Some(item.end_sec as i64),
                received_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::from_millis(0))
                    .as_millis() as u64,
            };

            if line.source_text.is_empty() && line.trans_text.is_empty() {
                continue;
            }
            let dedup_key = format!(
                "{:?}|{:?}|{}|{}",
                line.text_begin_time, line.text_end_time, line.source_text, line.trans_text
            );
            if existing.contains_key(&dedup_key) {
                continue;
            }
            existing.insert(dedup_key, ());
            lines_lock.push(line);
            inserted += 1;
        }

        Ok(inserted)
    }

    pub async fn get_subtitle(&self, sub_id: i64) -> Result<Vec<SubtitleContent>> {
        let url = format!(
            "https://yjapi.cmc.zju.edu.cn/courseapi/v3/web-socket/search-trans-result?sub_id={}&format=json",
            sub_id
        );
        let res = self.get(&url).send().await?;
        let json: SubtitleResponse = res.json().await?;

        if json.code != 0 {
            return Err(anyhow!("获取字幕失败，错误代码: {}", json.code));
        }

        if let Some(item) = json.list.first() {
            Ok(item
                .all_content
                .iter()
                .map(|c| SubtitleContent {
                    begin_sec: c.begin_sec,
                    end_sec: c.end_sec,
                    text: c.text.clone(),
                    trans_text: c.trans_text.clone(),
                })
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}
