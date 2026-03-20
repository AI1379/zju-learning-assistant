use crate::model::Upload;
use crate::zju_assist::ZjuAssist;
use percent_encoding::percent_decode_str;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub async fn get_uploads_list_core(
    zju_assist: Arc<Mutex<ZjuAssist>>,
    save_path: String,
    courses: Value,
    sync_upload: bool,
) -> Result<Vec<Upload>, String> {
    let zju_assist_guard = zju_assist.lock().await.clone();
    let mut all_uploads = Vec::new();
    let mut tasks: Vec<JoinHandle<Result<Vec<Upload>, String>>> = Vec::new();

    if let Some(courses_array) = courses.as_array() {
        for course in courses_array {
            let course_id = course["id"].as_i64().unwrap();
            let course_name = course["name"].as_str().unwrap().replace("/", "-");
            let zju_assist_clone = zju_assist_guard.clone();
            let save_path_clone = save_path.clone();

            tasks.push(tokio::task::spawn(async move {
                let mut uploads = Vec::new();
                let activities_uploads = zju_assist_clone
                    .get_activities_uploads(course_id)
                    .await
                    .map_err(|err| err.to_string())?;

                for upload in activities_uploads {
                    let id = upload["id"].as_i64().unwrap();
                    let reference_id = upload["reference_id"].as_i64().unwrap();
                    let file_name = upload["name"].as_str().unwrap().to_string();
                    let path = Path::new(&save_path_clone)
                        .join(&course_name)
                        .to_str()
                        .unwrap()
                        .to_string();
                    let size = upload["size"].as_u64().unwrap_or(1000);

                    uploads.push(Upload {
                        id,
                        reference_id,
                        file_name,
                        course_name: course_name.clone(),
                        path,
                        size,
                    });
                }
                Ok(uploads)
            }));
        }
    }

    for task in tasks {
        let uploads = task.await.map_err(|err| err.to_string())??;
        all_uploads.extend(uploads);
    }

    if sync_upload {
        let mut sync_uploads = Vec::new();
        for upload in all_uploads.iter() {
            let filepath = Path::new(&upload.path).join(&upload.file_name);

            if !filepath.exists()
                || filepath.metadata().map(|m| m.len()).unwrap_or(0) != upload.size
            {
                sync_uploads.push(upload.clone());
            }
        }
        all_uploads = sync_uploads;
    }

    Ok(all_uploads)
}

pub async fn download_upload_core<F>(
    zju_assist: Arc<Mutex<ZjuAssist>>,
    upload: Upload,
    sync_upload: bool,
    progress_callback: F,
) -> Result<(), String>
where
    F: Fn(u64, u64) + Send + 'static,
{
    let zju_assist_guard = zju_assist.lock().await.clone();
    let res = zju_assist_guard
        .get_uploads_response(upload.id, upload.reference_id)
        .await
        .map_err(|err| err.to_string())?;

    if !res.status().is_success() {
        return Err("下载失败".to_string());
    }

    std::fs::create_dir_all(Path::new(&upload.path)).map_err(|e| e.to_string())?;

    let content_length = res.content_length().unwrap_or(upload.size as u64);
    let mut file_name = upload.file_name.clone();
    let url = res.url().to_string();
    if let Some(start) = url.find("name=") {
        let start = start + 5;
        let end = url[start..].find("&").unwrap_or(url.len() - start);
        file_name = percent_decode_str(&url[start..start + end])
            .decode_utf8_lossy()
            .to_string();
    }
    let filepath = Path::new(&upload.path).join(&file_name);

    if sync_upload && filepath.exists() && filepath.metadata().unwrap().len() == content_length {
        progress_callback(content_length, content_length);
        return Ok(());
    }

    let mut file = tokio::fs::File::create(filepath.clone())
        .await
        .map_err(|e| e.to_string())?;

    let mut current_size: u64 = 0;
    use futures::TryStreamExt;
    use tokio::io::AsyncWriteExt;

    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.try_next().await.map_err(|e| e.to_string())? {
        current_size += chunk.len() as u64;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        progress_callback(current_size, content_length);
    }

    Ok(())
}
