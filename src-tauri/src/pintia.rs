use anyhow::{anyhow, Result};
use log::{debug, info};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{HeaderMap, ACCEPT, ORIGIN, REFERER};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use url::Url;

const PASSPORT_BASE: &str = "https://passport.pintia.cn";
const PINTIA_BASE: &str = "https://pintia.cn";

// pintia.cn rate-limits aggressively (HTTP 429 / RATE_LIMIT_EXCEEDED);
// 600ms between requests is the safe interval established by other clients
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(600);

// marker error so get_todos can distinguish an expired session from other failures
#[derive(Debug)]
struct SessionExpired;

impl std::fmt::Display for SessionExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pintia session expired")
    }
}

impl std::error::Error for SessionExpired {}

#[derive(Clone)]
pub struct PintiaAssist {
    jar: Arc<Jar>,
    throttle: Arc<Mutex<Instant>>,
    have_login: bool,
    // logged in via pasted PTASession cookie: no password to auto-relogin with
    cookie_login: bool,
    account: String,
    password: String,
}

impl PintiaAssist {
    pub fn new() -> Self {
        Self {
            jar: Arc::new(Jar::default()),
            throttle: Arc::new(Mutex::new(Instant::now() - MIN_REQUEST_INTERVAL)),
            have_login: false,
            cookie_login: false,
            account: String::new(),
            password: String::new(),
        }
    }

    pub fn is_login(&self) -> bool {
        self.have_login
    }

    pub fn get_account(&self) -> String {
        self.account.clone()
    }

    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "application/json;charset=UTF-8".parse().unwrap());
        headers.insert(ORIGIN, "https://pintia.cn".parse().unwrap());
        headers.insert(REFERER, "https://pintia.cn/".parse().unwrap());

        // built per request so the shared cookie jar can be swapped on logout/cookie login
        let client = Client::builder()
            .cookie_provider(Arc::clone(&self.jar))
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        client.request(method, url)
    }

    async fn throttle(&self) {
        let mut last = self.throttle.lock().await;
        if let Some(remaining) = MIN_REQUEST_INTERVAL.checked_sub(last.elapsed()) {
            tokio::time::sleep(remaining).await;
        }
        *last = Instant::now();
    }

    fn has_session_cookie(&self) -> bool {
        match Url::parse("https://pintia.cn/") {
            Ok(url) => self
                .jar
                .cookies(&url)
                .and_then(|value| value.to_str().ok().map(|s| s.to_string()))
                .map(|cookies| cookies.contains("PTASession="))
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// login with email/phone + password; returns the account identifier
    pub async fn login(&mut self, username: &str, password: &str) -> Result<String> {
        info!("pintia login: {}", username);
        self.throttle().await;

        let mut payload = json!({
            "password": password,
            "rememberMe": true,
            "inMiniProgram": false,
        });
        if username.contains('@') {
            payload["email"] = json!(username);
        } else {
            payload["phone"] = json!(username);
        }

        let res = self
            .request(Method::POST, &format!("{}/api/users/sessions", PASSPORT_BASE))
            .json(&payload)
            .send()
            .await?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            debug!("pintia login failed: {} {}", status, body);
            let error = serde_json::from_str::<Value>(&body).ok();
            let code = error
                .as_ref()
                .and_then(|v| v["error"]["code"].as_str())
                .unwrap_or("");
            if code == "GATEWAY_WRONG_CAPTCHA" {
                return Err(anyhow!(
                    "拼题A登录触发了人机验证（验证码）。请改用下方 Cookie 登录：在浏览器中登录 pintia.cn 后，按提示复制 PTASession Cookie 粘贴到此处。"
                ));
            }
            let message = error
                .as_ref()
                .and_then(|v| v["error"]["message"].as_str())
                .unwrap_or("");
            return Err(anyhow!(
                "拼题A登录失败: {}",
                if message.is_empty() {
                    format!("HTTP {}", status)
                } else {
                    message.to_string()
                }
            ));
        }

        if !self.has_session_cookie() {
            return Err(anyhow!("拼题A登录未返回会话 Cookie，请重试"));
        }

        self.have_login = true;
        self.cookie_login = false;
        self.account = username.to_string();
        self.password = password.to_string();
        Ok(self.account.clone())
    }

    /// login with a PTASession cookie value pasted by the user (captcha fallback)
    pub async fn login_with_cookie(&mut self, session: &str) -> Result<String> {
        let session = session.trim();
        let session = session
            .strip_prefix("PTASession=")
            .unwrap_or(session)
            .split(';')
            .next()
            .unwrap_or(session)
            .trim();
        if session.is_empty() {
            return Err(anyhow!("请粘贴 PTASession Cookie 的值"));
        }

        self.jar = Arc::new(Jar::default());
        let url = Url::parse("https://pintia.cn/")?;
        self.jar
            .add_cookie_str(&format!("PTASession={}; Domain=.pintia.cn; Path=/", session), &url);

        // verify the cookie actually works before accepting it
        if !self.probe_session().await? {
            return Err(anyhow!("Cookie 无效或已过期"));
        }

        self.have_login = true;
        self.cookie_login = true;
        self.password.clear();
        self.account = "Cookie 登录".to_string();
        Ok(self.account.clone())
    }

    pub fn logout(&mut self) {
        self.jar = Arc::new(Jar::default());
        self.have_login = false;
        self.cookie_login = false;
        self.account.clear();
        self.password.clear();
        info!("pintia logout");
    }

    /// probe whether the current PTASession is still accepted;
    /// unauthenticated /api/problem-sets returns 404 + USER_NOT_FOUND.
    /// 429 (rate limit) is treated as still-logged-in to avoid false logouts.
    async fn probe_session(&self) -> Result<bool> {
        self.throttle().await;
        let res = self
            .request(Method::GET, &format!("{}/api/problem-sets?limit=1", PINTIA_BASE))
            .send()
            .await?;
        let status = res.status();
        if status.is_success() || status.as_u16() == 429 {
            Ok(true)
        } else {
            debug!("pintia session probe status: {}", status);
            Ok(false)
        }
    }

    /// returns Some(account) when logged in with a working session,
    /// re-logging in automatically if we hold the password; None when logged out
    pub async fn check_login(&mut self) -> Result<Option<String>> {
        if !self.have_login {
            return Ok(None);
        }
        if self.probe_session().await? {
            return Ok(Some(self.account.clone()));
        }
        if self.cookie_login || self.password.is_empty() {
            self.have_login = false;
            return Ok(None);
        }
        info!("pintia session expired, re-login");
        match self.login(&self.account.clone(), &self.password.clone()).await {
            Ok(account) => Ok(Some(account)),
            Err(err) => {
                self.have_login = false;
                Err(err)
            }
        }
    }

    /// problem sets whose deadline (endAt) is still in the future = active homework/exams
    pub async fn get_problem_sets(&self) -> Result<Vec<Value>> {
        let filter = format!(
            "{{\"endAtAfter\":\"{}\"}}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ")
        );
        self.get_problem_sets_raw(&filter).await
    }

    async fn get_problem_sets_raw(&self, filter: &str) -> Result<Vec<Value>> {
        self.throttle().await;
        let url = format!("{}/api/problem-sets", PINTIA_BASE);
        let mut res = self
            .request(Method::GET, &url)
            .query(&[
                ("filter", filter),
                ("limit", "100"),
                ("order_by", "END_AT"),
                ("asc", "true"),
            ])
            .send()
            .await?;

        // rate limited: wait and retry once
        if res.status().as_u16() == 429 {
            info!("pintia rate limited, retrying in 3s");
            tokio::time::sleep(Duration::from_secs(3)).await;
            res = self
                .request(Method::GET, &url)
                .query(&[
                    ("filter", filter),
                    ("limit", "100"),
                    ("order_by", "END_AT"),
                    ("asc", "true"),
                ])
                .send()
                .await?;
        }

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            debug!("pintia problem-sets failed: {} {}", status, body);
            if status.as_u16() == 404 && body.contains("USER_NOT_FOUND") {
                return Err(SessionExpired.into());
            }
            return Err(anyhow!("获取拼题A题集失败: HTTP {}", status));
        }

        let json: Value = res.json().await?;
        Ok(json["problemSets"].as_array().cloned().unwrap_or_default())
    }

    /// fetch sets with the given filter, re-logging in once when the session
    /// expired and we hold the password
    async fn fetch_sets_with_relogin(&mut self, filter: &str) -> Result<Vec<Value>> {
        if !self.have_login {
            return Err(anyhow!("Not login"));
        }

        match self.get_problem_sets_raw(filter).await {
            Ok(sets) => Ok(sets),
            Err(err) => {
                if err.downcast_ref::<SessionExpired>().is_none() {
                    return Err(err);
                }
                if self.cookie_login || self.password.is_empty() {
                    self.have_login = false;
                    return Err(anyhow!("拼题A会话已过期，请重新登录"));
                }
                info!("pintia session expired, re-login");
                self.login(&self.account.clone(), &self.password.clone())
                    .await?;
                self.get_problem_sets_raw(filter).await
            }
        }
    }

    /// active pintia assignments mapped into the app's todo item shape
    pub async fn get_todos(&mut self) -> Result<Vec<Value>> {
        let filter = format!(
            "{{\"endAtAfter\":\"{}\"}}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ")
        );
        let sets = self.fetch_sets_with_relogin(&filter).await?;

        let mut todos = Vec::new();
        for set in sets {
            let end_time = match set["endAt"].as_str() {
                Some(end) => end.to_string(),
                None => continue, // always-available sets are not deadlines
            };
            let set_id = set["id"]
                .as_str()
                .and_then(|id| id.parse::<i64>().ok())
                .or_else(|| set["id"].as_i64())
                .unwrap_or(0);
            let set_type = set["type"].as_str().unwrap_or("");
            if set_type == "BOOK" {
                continue;
            }
            let mut title = set["name"].as_str().unwrap_or("未命名题集").to_string();
            match set_type {
                "EXAM" => title.push_str("（考试）"),
                "CONTEST" => title.push_str("（竞赛）"),
                _ => {}
            }
            todos.push(json!({
                "source": "pintia",
                "id": set_id,
                "course_id": set_id,
                "title": title,
                "course_name": set["organizationName"].as_str().unwrap_or("拼题A"),
                "end_time": end_time,
            }));
        }
        Ok(todos)
    }

    /// all problem sets (ongoing + ended), raw API shape, for the pintia page
    pub async fn get_assignments(&mut self) -> Result<Vec<Value>> {
        self.fetch_sets_with_relogin("{}").await
    }
}
