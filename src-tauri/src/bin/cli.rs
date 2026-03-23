use anyhow::{anyhow, Result};

#[path = "../logic.rs"]
mod logic;
#[path = "../model.rs"]
mod model;
#[path = "../utils/mod.rs"]
mod utils;
#[path = "../zju_assist.rs"]
mod zju_assist;

use clap::{ArgAction, Parser, Subcommand};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use logic::{download_upload_core, get_uploads_list_core};
use model::Upload;
use utils::images_to_pdf;
use zju_assist::ZjuAssist;

const KEYRING_SERVICE: &str = "zju-assist-cli";
const CONFIG_FILE_NAME: &str = "cli-config.json";
const CREDENTIAL_FILE_NAME: &str = "cli-credentials.json";

#[derive(Parser)]
#[command(name = "zju-assist-cli")]
#[command(about = "CLI tool for ZJU Learning Assistant", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Login to ZJU account
    Login {
        #[arg(short, long)]
        username: Option<String>,
        /// Password in plain text. Prefer omitting this and entering password interactively.
        #[arg(short, long)]
        password: Option<String>,
        /// Read password from stdin (for CI / server scripts).
        #[arg(long, default_value_t = false)]
        password_stdin: bool,
        /// Save password in keyring for later non-interactive commands.
        #[arg(long, default_value_t = false)]
        save_password: bool,
    },
    /// List pending todo items
    Todo,
    /// List my courses
    Courses,
    /// List my classroom (Zhiyun Classroom) courses.
    ClassroomCourses {
        /// Optional keyword filter for course name.
        #[arg(short, long, default_value = "")]
        keyword: String,
        /// Optional teacher filter.
        #[arg(short = 't', long, default_value = "")]
        teacher: String,
    },
    /// List all classroom sessions under one classroom course.
    ClassroomSubs {
        #[arg(long)]
        classroom_course_id: i64,
    },
    /// Download course uploads from courses.zju.edu.cn
    Download {
        /// Download files for a specific course ID. If omitted, downloads for all courses.
        #[arg(short, long)]
        course_id: Option<i64>,
        /// Only download files that are not already present or have size mismatch
        #[arg(short, long, default_value_t = false)]
        sync: bool,
        /// Path to save downloads
        #[arg(short, long)]
        path: Option<String>,
    },
    /// List all downloadable files under a specific course.
    Files {
        #[arg(short, long)]
        course_id: i64,
    },
    /// Download one file by course id + file id, or by course id + unique file name.
    DownloadFile {
        #[arg(short, long)]
        course_id: i64,
        /// Upload file id from `files` command.
        #[arg(long)]
        file_id: Option<i64>,
        /// File name to search. If multiple files match, command will return candidates.
        #[arg(long)]
        name: Option<String>,
        /// Path to save downloads.
        #[arg(short, long)]
        path: Option<String>,
        /// Skip download if local file exists and size matches.
        #[arg(short, long, default_value_t = false)]
        sync: bool,
    },
    /// Download classroom PPT images and optional PDF.
    Ppt {
        /// Classroom course id (Zhiyun Classroom course id).
        #[arg(long)]
        classroom_course_id: i64,
        /// Download only one sub-session if provided.
        #[arg(long)]
        sub_id: Option<i64>,
        /// Base path to save downloads.
        #[arg(short, long)]
        path: Option<String>,
        /// Whether to generate PDF after downloading PPT images.
        #[arg(long, default_value_t = true, action = ArgAction::Set)]
        to_pdf: bool,
    },
    /// Download classroom ASR transcript text.
    Asr {
        /// Classroom course id (Zhiyun Classroom course id).
        #[arg(long)]
        classroom_course_id: i64,
        /// Download only one sub-session if provided.
        #[arg(long)]
        sub_id: Option<i64>,
        /// Base path to save downloads.
        #[arg(short, long)]
        path: Option<String>,
    },
    /// Configure dedicated proxy for zju-learning-assistant requests.
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
}

#[derive(Subcommand)]
enum ProxyCommand {
    /// Set dedicated proxy URL, for example http://127.0.0.1:7890
    Set { url: String },
    /// Remove dedicated proxy and return to default behavior.
    Unset,
    /// Show current dedicated proxy setting.
    Show,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct CliConfig {
    username: Option<String>,
    proxy: Option<String>,
    save_password: bool,
    credential_backend: CredentialBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum CredentialBackend {
    #[default]
    Auto,
    Keyring,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredential {
    username: String,
    password: String,
}

fn cli_config_dir() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            return Ok(Path::new(&appdata).join("zju-learning-assistant"));
        }
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        return Ok(Path::new(&xdg).join("zju-learning-assistant"));
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(Path::new(&home)
            .join(".config")
            .join("zju-learning-assistant"));
    }

    Ok(env::current_dir()?.join(".zju-learning-assistant"))
}

fn cli_config_path() -> Result<PathBuf> {
    Ok(cli_config_dir()?.join(CONFIG_FILE_NAME))
}

fn load_cli_config() -> Result<CliConfig> {
    let path = cli_config_path()?;
    if !path.exists() {
        return Ok(CliConfig::default());
    }

    let content = fs::read_to_string(path)?;
    let mut config = serde_json::from_str::<CliConfig>(&content)?;
    if matches!(config.credential_backend, CredentialBackend::Auto) {
        config.credential_backend = default_credential_backend();
    }
    Ok(config)
}

fn default_credential_backend() -> CredentialBackend {
    #[cfg(target_os = "linux")]
    {
        // Linux server environments often have no working secret-service daemon.
        CredentialBackend::File
    }

    #[cfg(not(target_os = "linux"))]
    {
        CredentialBackend::Keyring
    }
}

fn credentials_file_path() -> Result<PathBuf> {
    Ok(cli_config_dir()?.join(CREDENTIAL_FILE_NAME))
}

fn store_password_file(username: &str, password: &str) -> Result<()> {
    let path = credentials_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = StoredCredential {
        username: username.to_string(),
        password: password.to_string(),
    };
    let content = serde_json::to_string_pretty(&payload)?;
    fs::write(&path, content)?;

    #[cfg(target_os = "linux")]
    {
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&path, permissions)?;
    }

    Ok(())
}

fn read_password_file(username: &str) -> Result<Option<String>> {
    let path = credentials_file_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    let payload = serde_json::from_str::<StoredCredential>(&content)?;
    if payload.username != username {
        return Ok(None);
    }
    if payload.password.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(payload.password))
}

fn remove_password_file() -> Result<()> {
    let path = credentials_file_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn save_cli_config(config: &CliConfig) -> Result<()> {
    let path = cli_config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content)?;
    Ok(())
}

fn load_password(username: &str, config: &CliConfig) -> Result<String> {
    if let Ok(password) = env::var("ZLA_PASSWORD") {
        if !password.trim().is_empty() {
            return Ok(password);
        }
    }

    if !config.save_password {
        return Err(anyhow!(
            "No password available. Use 'login --save-password', or provide ZLA_PASSWORD env."
        ));
    }

    match config.credential_backend {
        CredentialBackend::Keyring => {
            let entry = Entry::new(KEYRING_SERVICE, username)?;
            let password = entry
                .get_password()
                .map_err(|err| anyhow!("Cannot read password from keyring: {err}"))?;
            if password.trim().is_empty() {
                return Err(anyhow!(
                    "Empty password in keyring. Please run login again and refresh credentials."
                ));
            }
            Ok(password)
        }
        CredentialBackend::File => {
            let password = read_password_file(username)?
                .ok_or(anyhow!("No stored password in credential file for this user."))?;
            Ok(password)
        }
        CredentialBackend::Auto => {
            // Auto should be resolved in load_cli_config, keep a safe fallback here.
            let entry = Entry::new(KEYRING_SERVICE, username)?;
            let password = entry
                .get_password()
                .map_err(|err| anyhow!("Cannot read password from keyring: {err}"))?;
            if password.trim().is_empty() {
                return Err(anyhow!("Empty password in keyring."));
            }
            Ok(password)
        }
    }
}

fn persist_password(
    username: &str,
    password: &str,
    save_password: bool,
    backend: &CredentialBackend,
) -> Result<CredentialBackend> {
    if !save_password {
        if let Ok(entry) = Entry::new(KEYRING_SERVICE, username) {
            let _ = entry.delete_password();
        }
        let _ = remove_password_file();
        return Ok(backend.clone());
    }

    match backend {
        CredentialBackend::Keyring => {
            let entry = Entry::new(KEYRING_SERVICE, username)?;
            entry
                .set_password(password)
                .map_err(|err| anyhow!("Cannot save password to keyring: {err}"))?;
            let _ = remove_password_file();
            Ok(CredentialBackend::Keyring)
        }
        CredentialBackend::File => {
            store_password_file(username, password)?;
            if let Ok(entry) = Entry::new(KEYRING_SERVICE, username) {
                let _ = entry.delete_password();
            }
            Ok(CredentialBackend::File)
        }
        CredentialBackend::Auto => {
            let resolved = default_credential_backend();
            persist_password(username, password, save_password, &resolved)
        }
    }
}

fn read_password_interactive() -> Result<String> {
    let password = rpassword::prompt_password("Password: ")?;
    if password.trim().is_empty() {
        return Err(anyhow!("Password cannot be empty."));
    }
    Ok(password)
}

fn read_password_stdin() -> Result<String> {
    use std::io::{self, Read};

    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    let password = buffer.trim_end_matches(['\r', '\n']).to_string();
    if password.trim().is_empty() {
        return Err(anyhow!("Password from stdin cannot be empty."));
    }
    Ok(password)
}

fn build_assist_from_config(config: &CliConfig) -> Result<ZjuAssist> {
    let mut assist = ZjuAssist::new();
    assist.set_custom_proxy(config.proxy.clone())?;
    Ok(assist)
}

async fn get_assist_with_session(config: &CliConfig) -> Result<ZjuAssist> {
    let username = config
        .username
        .as_deref()
        .ok_or(anyhow!("No saved username. Please run 'login' first."))?;
    let password = load_password(username, config)?;

    let mut assist = build_assist_from_config(config)?;
    assist
        .login(username, &password)
        .await
        .map_err(|e| anyhow!("Login failed: {e}"))?;

    Ok(assist)
}

async fn download_ppt_for_sub(
    assist: &ZjuAssist,
    base_path: &str,
    course_name: &str,
    sub_name: &str,
    urls: Vec<String>,
    to_pdf: bool,
) -> Result<()> {
    let root = Path::new(base_path).join(course_name).join(sub_name);
    let image_dir = root.join("ppt_images");
    let mut image_paths = Vec::new();

    for (index, url) in urls.iter().enumerate() {
        let ext = url
            .split('.')
            .next_back()
            .filter(|s| !s.is_empty())
            .unwrap_or("jpg");
        let image_path = image_dir.join(format!("{}.{}", index + 1, ext));
        let image_path_str = image_path.to_string_lossy().to_string();
        assist.download_ppt_image(url, &image_path_str).await?;
        image_paths.push(image_path_str);

        println!(
            "[{}/{}] {} - {}",
            index + 1,
            urls.len(),
            course_name,
            sub_name
        );
    }

    if to_pdf && !image_paths.is_empty() {
        let pdf_path = root.join(format!("{}-{}.pdf", course_name, sub_name));
        let pdf_path_str = pdf_path.to_string_lossy().to_string();
        images_to_pdf(image_paths, &pdf_path_str)
            .map_err(|err| anyhow!("Failed to generate PDF: {err}"))?;
    }

    Ok(())
}

async fn download_asr_for_sub(
    assist: &ZjuAssist,
    base_path: &str,
    course_name: &str,
    sub_name: &str,
    sub_id: i64,
) -> Result<()> {
    let target_dir = Path::new(base_path).join(course_name).join(sub_name);
    tokio::fs::create_dir_all(&target_dir).await?;

    let contents = assist.get_subtitle(sub_id).await?;
    let transcript = contents
        .iter()
        .map(|item| item.text.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    let output_path = target_dir.join("asr_text.txt");
    tokio::fs::write(&output_path, transcript).await?;
    Ok(())
}

async fn get_course_uploads(
    assist: &ZjuAssist,
    course_id: i64,
    base_path: &str,
) -> Result<Vec<Upload>> {
    let courses = assist.get_courses().await?;
    let course = courses
        .iter()
        .find(|course| course["id"].as_i64() == Some(course_id))
        .ok_or(anyhow!("Course {} not found in your current course list.", course_id))?;

    let course_name = course["name"].as_str().unwrap_or("Unknown").replace('/', "-");
    let course_path = Path::new(base_path)
        .join(&course_name)
        .to_string_lossy()
        .to_string();

    let mut raw_uploads = assist.get_activities_uploads(course_id).await?;
    raw_uploads.extend(assist.get_homework_uploads(course_id).await?);

    let mut seen = HashSet::new();
    let mut uploads = Vec::new();
    for upload in raw_uploads {
        let id = match upload["id"].as_i64() {
            Some(v) => v,
            None => continue,
        };
        let reference_id = match upload["reference_id"].as_i64() {
            Some(v) => v,
            None => continue,
        };
        let file_name = upload["name"].as_str().unwrap_or("Unnamed").to_string();
        let size = upload["size"].as_u64().unwrap_or(0);

        if !seen.insert((id, reference_id)) {
            continue;
        }

        uploads.push(Upload {
            id,
            reference_id,
            file_name,
            course_name: course_name.clone(),
            path: course_path.clone(),
            size,
        });
    }

    uploads.sort_by(|a, b| {
        a.file_name
            .cmp(&b.file_name)
            .then(a.id.cmp(&b.id))
            .then(a.reference_id.cmp(&b.reference_id))
    });

    Ok(uploads)
}

async fn get_classroom_courses(
    assist: &ZjuAssist,
    keyword: &str,
    teacher: &str,
) -> Result<Vec<(i64, String, String, String)>> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();

    if !keyword.trim().is_empty() || !teacher.trim().is_empty() {
        let courses = assist.search_courses(keyword, teacher).await?;
        for item in courses {
            let cid = match item["course_id"].as_i64() {
                Some(v) => v,
                None => continue,
            };
            if !seen.insert(cid) {
                continue;
            }
            let title = item["title"].as_str().unwrap_or("Unknown").to_string();
            let teacher = item["realname"].as_str().unwrap_or("").to_string();
            let term = item["term_name"].as_str().unwrap_or("").to_string();
            merged.push((cid, title, teacher, term));
        }
        merged.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        return Ok(merged);
    }

    // No keyword: infer classroom courses from current learning-course names.
    let learning_courses = assist.get_courses().await?;
    for course in learning_courses {
        let name = match course["name"].as_str() {
            Some(v) if !v.trim().is_empty() => v.to_string(),
            _ => continue,
        };
        let courses = match assist.search_courses(&name, "").await {
            Ok(v) => v,
            Err(_) => continue,
        };
        for item in courses {
            let cid = match item["course_id"].as_i64() {
                Some(v) => v,
                None => continue,
            };
            if !seen.insert(cid) {
                continue;
            }
            let title = item["title"].as_str().unwrap_or("Unknown").to_string();
            let teacher = item["realname"].as_str().unwrap_or("").to_string();
            let term = item["term_name"].as_str().unwrap_or("").to_string();
            merged.push((cid, title, teacher, term));
        }
    }

    merged.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    Ok(merged)
}

fn select_upload(
    uploads: &[Upload],
    file_id: Option<i64>,
    name: Option<String>,
) -> Result<Upload> {
    match (file_id, name) {
        (Some(_), Some(_)) => Err(anyhow!(
            "Please provide either --file-id or --name, not both."
        )),
        (None, None) => Err(anyhow!("Please provide --file-id or --name.")),
        (Some(target_id), None) => {
            let candidates = uploads
                .iter()
                .filter(|u| u.id == target_id)
                .cloned()
                .collect::<Vec<_>>();

            if candidates.is_empty() {
                return Err(anyhow!("File id {} not found in this course.", target_id));
            }
            if candidates.len() > 1 {
                let details = candidates
                    .iter()
                    .map(|u| format!("id={} ref={} name={}", u.id, u.reference_id, u.file_name))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Err(anyhow!(
                    "File id {} is not unique in this course. Candidates:\n{}",
                    target_id,
                    details
                ));
            }

            Ok(candidates[0].clone())
        }
        (None, Some(target_name)) => {
            let exact = uploads
                .iter()
                .filter(|u| u.file_name == target_name)
                .cloned()
                .collect::<Vec<_>>();

            let candidates = if exact.is_empty() {
                let key = target_name.to_lowercase();
                uploads
                    .iter()
                    .filter(|u| u.file_name.to_lowercase().contains(&key))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                exact
            };

            if candidates.is_empty() {
                return Err(anyhow!("No file matched name '{}'.", target_name));
            }
            if candidates.len() > 1 {
                let details = candidates
                    .iter()
                    .map(|u| format!("id={} ref={} name={}", u.id, u.reference_id, u.file_name))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Err(anyhow!(
                    "Multiple files matched name '{}'. Please use --file-id. Candidates:\n{}",
                    target_name,
                    details
                ));
            }

            Ok(candidates[0].clone())
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Login {
            username,
            password,
            password_stdin,
            save_password,
        } => {
            let mut config = load_cli_config()?;
            let username = username
                .or(config.username.clone())
                .ok_or(anyhow!("Username is required for first login."))?;
            let password = if password_stdin {
                if password.is_some() {
                    return Err(anyhow!(
                        "Use either --password or --password-stdin, not both."
                    ));
                }
                read_password_stdin()?
            } else {
                match password {
                    Some(value) => value,
                    None => read_password_interactive()?,
                }
            };

            let mut assist = build_assist_from_config(&config)?;
            println!("Attempting to login...");
            assist
                .login(&username, &password)
                .await
                .map_err(|e| anyhow!("Login failed: {}", e))?;

            config.username = Some(username.clone());
            config.save_password = save_password;
            if matches!(config.credential_backend, CredentialBackend::Auto) {
                config.credential_backend = default_credential_backend();
            }
            if save_password {
                #[cfg(target_os = "linux")]
                {
                    // Linux headless environments are often more stable with file backend.
                    config.credential_backend = CredentialBackend::File;
                }
            }

            let actual_backend = persist_password(
                &username,
                &password,
                save_password,
                &config.credential_backend,
            )?;
            config.credential_backend = actual_backend;
            save_cli_config(&config)?;

            println!("Successfully logged in as {}", username);
            if save_password {
                match config.credential_backend {
                    CredentialBackend::Keyring => {
                        println!("Password saved in keyring for future commands.");
                    }
                    CredentialBackend::File => {
                        println!(
                            "Password saved to local credential file (Linux fallback, file mode 600)."
                        );
                    }
                    CredentialBackend::Auto => {
                        println!("Password saved using auto backend.");
                    }
                }
            } else {
                println!("Password not saved. Use ZLA_PASSWORD env or login again next time.");
            }
        }
        Commands::Todo => {
            let config = load_cli_config()?;
            let assist = get_assist_with_session(&config).await?;
            let todos = assist.get_todo_list().await?;
            println!("{:<10} {:<30} {:<20}", "ID", "Title", "Deadline");
            for todo in todos {
                let id = todo["id"].as_i64().unwrap_or(0);
                let title = todo["title"].as_str().unwrap_or("Unknown");
                let end_time = todo["end_time"].as_str().unwrap_or("No deadline");
                println!("{:<10} {:<30} {:<20}", id, title, end_time);
            }
        }
        Commands::Courses => {
            let config = load_cli_config()?;
            let assist = get_assist_with_session(&config).await?;
            let courses = assist.get_courses().await?;
            println!("{:<10} {:<50}", "ID", "Course Name");
            for course in courses {
                let id = course["id"].as_i64().unwrap_or(0);
                let name = course["name"].as_str().unwrap_or("Unknown");
                println!("{:<10} {:<50}", id, name);
            }
        }
        Commands::ClassroomCourses { keyword, teacher } => {
            let config = load_cli_config()?;
            let mut assist = get_assist_with_session(&config).await?;
            assist.keep_classroom_alive().await?;
            let courses = get_classroom_courses(&assist, &keyword, &teacher).await?;

            if courses.is_empty() {
                println!("No classroom courses found.");
                return Ok(());
            }

            println!(
                "{:<14} {:<40} {:<20} {:<20}",
                "Classroom ID", "Course Name", "Teacher", "Term"
            );
            for (cid, title, teacher, term) in courses {
                println!("{:<14} {:<40} {:<20} {:<20}", cid, title, teacher, term);
            }
        }
        Commands::ClassroomSubs { classroom_course_id } => {
            let config = load_cli_config()?;
            let mut assist = get_assist_with_session(&config).await?;
            assist.keep_classroom_alive().await?;
            let subs = assist.get_course_subs(classroom_course_id).await?;

            if subs.is_empty() {
                println!("No classroom sessions found for course {}.", classroom_course_id);
                return Ok(());
            }

            println!("{:<12} {:<40} {:<30}", "Sub ID", "Course Name", "Sub Name");
            for sub in subs {
                println!(
                    "{:<12} {:<40} {:<30}",
                    sub.sub_id, sub.course_name, sub.sub_name
                );
            }
        }
        Commands::Download {
            course_id,
            sync,
            path,
        } => {
            let config = load_cli_config()?;
            let assist = Arc::new(Mutex::new(get_assist_with_session(&config).await?));

            let save_path = path.unwrap_or_else(|| "Downloads".to_string());
            let courses_val = {
                let assist_guard = assist.lock().await;
                let mut courses = assist_guard.get_courses().await?;
                if let Some(cid) = course_id {
                    courses.retain(|course| course["id"].as_i64() == Some(cid));
                }
                serde_json::to_value(courses)?
            };

            let uploads = get_uploads_list_core(assist.clone(), save_path, courses_val, sync)
                .await
                .map_err(|err| anyhow!(err))?;

            println!("Found {} files to download.", uploads.len());
            for upload in uploads {
                println!("Downloading {}...", upload.file_name);
                let assist_clone = assist.clone();
                download_upload_core(assist_clone, upload, sync, |_curr, _total| {})
                    .await
                    .map_err(|err| anyhow!(err))?;
            }
            println!("Download complete.");
        }
        Commands::Files { course_id } => {
            let config = load_cli_config()?;
            let assist = get_assist_with_session(&config).await?;
            let base_path = "Downloads".to_string();
            let uploads = get_course_uploads(&assist, course_id, &base_path).await?;

            if uploads.is_empty() {
                println!("No downloadable files found for course {}.", course_id);
                return Ok(());
            }

            println!(
                "{:<10} {:<14} {:>10} {:<}",
                "File ID", "Reference ID", "Size", "Name"
            );
            for upload in uploads {
                println!(
                    "{:<10} {:<14} {:>10} {:<}",
                    upload.id, upload.reference_id, upload.size, upload.file_name
                );
            }
        }
        Commands::DownloadFile {
            course_id,
            file_id,
            name,
            path,
            sync,
        } => {
            let config = load_cli_config()?;
            let assist = get_assist_with_session(&config).await?;
            let base_path = path.unwrap_or_else(|| "Downloads".to_string());
            let uploads = get_course_uploads(&assist, course_id, &base_path).await?;
            let selected = select_upload(&uploads, file_id, name)?;

            println!(
                "Selected file: id={} ref={} name={}",
                selected.id, selected.reference_id, selected.file_name
            );

            let assist = Arc::new(Mutex::new(assist));
            download_upload_core(assist, selected, sync, |_curr, _total| {})
                .await
                .map_err(|err| anyhow!(err))?;
            println!("Download complete.");
        }
        Commands::Ppt {
            classroom_course_id,
            sub_id,
            path,
            to_pdf,
        } => {
            let config = load_cli_config()?;
            let mut assist = get_assist_with_session(&config).await?;
            assist.keep_classroom_alive().await?;

            let base_path = path.unwrap_or_else(|| "Downloads".to_string());
            let mut subs = assist.get_course_subs(classroom_course_id).await?;
            if let Some(target_sub_id) = sub_id {
                subs.retain(|sub| sub.sub_id == target_sub_id);
            }

            if subs.is_empty() {
                return Err(anyhow!(
                    "No classroom sessions found for classroom course {}.",
                    classroom_course_id
                ));
            }

            for sub in subs {
                println!(
                    "Fetching PPT urls: {} - {} (classroom_course_id={})",
                    sub.course_name, sub.sub_name, classroom_course_id
                );
                let urls = assist.get_ppt_urls(sub.course_id, sub.sub_id).await?;
                if urls.is_empty() {
                    println!("Skip {} - {}: no PPT found.", sub.course_name, sub.sub_name);
                    continue;
                }

                println!(
                    "Downloading {} PPT images for {} - {}",
                    urls.len(),
                    sub.course_name,
                    sub.sub_name
                );
                download_ppt_for_sub(
                    &assist,
                    &base_path,
                    &sub.course_name,
                    &sub.sub_name,
                    urls,
                    to_pdf,
                )
                .await?;
                println!("Done {} - {}", sub.course_name, sub.sub_name);
            }
        }
        Commands::Asr {
            classroom_course_id,
            sub_id,
            path,
        } => {
            let config = load_cli_config()?;
            let mut assist = get_assist_with_session(&config).await?;
            assist.keep_classroom_alive().await?;

            let base_path = path.unwrap_or_else(|| "Downloads".to_string());
            let mut subs = assist.get_course_subs(classroom_course_id).await?;
            if let Some(target_sub_id) = sub_id {
                subs.retain(|sub| sub.sub_id == target_sub_id);
            }

            if subs.is_empty() {
                return Err(anyhow!(
                    "No classroom sessions found for classroom course {}.",
                    classroom_course_id
                ));
            }

            for sub in subs {
                println!(
                    "Downloading ASR text: {} - {} (classroom_course_id={})",
                    sub.course_name, sub.sub_name, classroom_course_id
                );
                download_asr_for_sub(
                    &assist,
                    &base_path,
                    &sub.course_name,
                    &sub.sub_name,
                    sub.sub_id,
                )
                .await?;
                println!("Done {} - {}", sub.course_name, sub.sub_name);
            }
        }
        Commands::Proxy { command } => {
            let mut config = load_cli_config()?;
            match command {
                ProxyCommand::Set { url } => {
                    let mut assist = ZjuAssist::new();
                    assist.set_custom_proxy(Some(url.clone()))?;
                    config.proxy = Some(url.clone());
                    save_cli_config(&config)?;
                    println!("Dedicated proxy configured: {}", url);
                }
                ProxyCommand::Unset => {
                    config.proxy = None;
                    save_cli_config(&config)?;
                    println!("Dedicated proxy removed.");
                }
                ProxyCommand::Show => {
                    if let Some(proxy) = config.proxy {
                        println!("Dedicated proxy: {}", proxy);
                    } else {
                        println!("Dedicated proxy is not set.");
                    }
                }
            }
        }
    }

    Ok(())
}
