use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub save_path: String,
    pub to_pdf: bool,
    pub auto_download: bool,
    pub excluded_upload_extensions: Vec<String>,
    pub ding_url: String,
    pub auto_open_download_list: bool,
    pub tray: bool,
    pub max_concurrent_tasks: u32,
    pub auto_start: bool,
    pub mail_notifications: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub mail_recipient: String,

    pub download_subtitle: bool,
    pub subtitle_language: Vec<String>,
    pub subtitle_format: String,
    pub subtitle_with_timestamps: bool,

    pub llm_enabled: bool,
    pub llm_api_base: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_temperature: f64,
    pub llm_prompt: String,
    pub llm_hide_think_tag: bool,

    pub show_live_capture_controls: bool,
    pub live_capture_auto_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            save_path: "Downloads".to_string(),
            to_pdf: true,
            auto_download: true,
            excluded_upload_extensions: Vec::new(),
            ding_url: String::new(),
            auto_open_download_list: true,
            tray: true,
            max_concurrent_tasks: 3,
            auto_start: false,

            mail_notifications: false,
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_username: String::new(),
            smtp_password: String::new(),
            mail_recipient: String::new(),

            download_subtitle: false,
            subtitle_language: vec!["zh".to_string()],
            subtitle_format: "srt".to_string(),
            subtitle_with_timestamps: true,

            llm_enabled: false,
            llm_api_base: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_temperature: 0.2,
            llm_prompt: "你是一个专业的课程助教。请根据提供的课程字幕内容，总结课程的核心知识点、重点和难点。输出格式要求清晰、结构化，使用 Markdown 格式。".to_string(),
            llm_hide_think_tag: true,

            show_live_capture_controls: true,
            live_capture_auto_start: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Upload {
    pub id: i64,
    pub reference_id: i64,
    pub file_name: String,
    pub course_name: String,
    pub path: String, // actual save path is path + file_name
    pub size: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct Progress {
    pub id: String,
    pub status: String,
    pub file_name: String,
    pub downloaded_size: u64,
    pub total_size: u64,
    #[serde(default)]
    pub msg: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Subject {
    pub course_id: i64,
    pub sub_id: i64,
    pub course_name: String,
    pub sub_name: String,
    pub lecturer_name: String,
    pub path: String, // actual save path is path + sub_name
    pub ppt_image_urls: Vec<String>,
    pub start_at: Option<i64>,
    pub room: Option<String>,
    pub tenant_code: Option<String>,
    pub sub_public: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LiveTranscriptLine {
    pub source_text: String,
    pub trans_text: String,
    pub text_begin_time: Option<i64>,
    pub text_end_time: Option<i64>,
    pub end_time: Option<i64>,
    pub received_at_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LiveTranscriptSessionStatus {
    pub course_id: i64,
    pub sub_id: i64,
    pub ws_url: String,
    pub started_at_ms: u64,
    pub line_count: usize,
    pub is_running: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
}
