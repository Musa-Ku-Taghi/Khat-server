use std::sync::Arc;

use chrono::{Duration, Local};
use rand::Rng;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::{Database, DbError, DbService, FileMetadata};
use crate::models::{FileUploadResponse, ResponseStatus, UploadType};
use crate::validators::validate_file_hash;

pub async fn handle_file_upload_request(
    db: Arc<Database>,
    _username: String,
    hash: String,
    name: String,
    size: u64,
    config: &Config,
    requester_ip: &str,
) -> FileUploadResponse {
    if size > config.file_max_size_bytes {
        warn!("File upload denied: size {size} exceeds limit");
        return error_response(ResponseStatus::PayloadTooLarge, "File too large");
    }

    if let Err(e) = validate_file_hash(&hash) {
        warn!("Invalid hash format: {e}");
        return error_response(ResponseStatus::InvalidInput, e);
    }

    let existing: Result<Option<FileMetadata>, DbError> = db
        .call({
            let hash = hash.clone();
            move |d| d.get_file_by_hash(&hash)
        })
        .await;

    match existing {
        Ok(Some(meta)) if !meta.is_temp => {
            let thumb_url = meta.thumb_path.map(|_| format!("/thumb/{}", meta.id));
            info!("File upload duplicate: hash {}, file_id {}", hash, meta.id);
            FileUploadResponse {
                msg_type: "file_upload_response".to_string(),
                status: ResponseStatus::Duplicate,
                upload_url: None,
                file_id: Some(meta.id),
                thumb_url,
                name: Some(name),
                message: None,
            }
        }
        Ok(Some(_)) => {
            warn!("File upload pending: hash {hash} already in progress");
            FileUploadResponse {
                msg_type: "file_upload_response".to_string(),
                status: ResponseStatus::Pending,
                upload_url: None,
                file_id: None,
                thumb_url: None,
                name: None,
                message: Some("Upload already in progress".into()),
            }
        }
        Ok(None) => create_new_temp_upload(db, hash, name, size, config, requester_ip).await,
        Err(e) => {
            error!("Error checking hash: {e}");
            error_response(ResponseStatus::Error, "Database error")
        }
    }
}

async fn create_new_temp_upload(
    db: Arc<Database>,
    hash: String,
    name: String,
    size: u64,
    config: &Config,
    requester_ip: &str,
) -> FileUploadResponse {
    let token = {
        let mut rng = rand::thread_rng();
        let mut bytes = vec![0u8; config.temp_token_bytes];
        rng.fill(&mut bytes[..]);
        hex::encode(bytes)
    };
    let expires_at =
        Local::now().naive_local() + Duration::seconds(config.file_upload_temp_expiry_secs as i64);
    let temp_dir = config.temp_upload_dir.clone();
    let requester_ip = requester_ip.to_string();

    let result = db
        .call({
            let hash = hash.clone();
            let name = name.clone();
            let token = token.clone();
            move |d| {
                d.create_temp_upload(
                    &hash,
                    &name,
                    "unknown",
                    size,
                    &token,
                    expires_at,
                    None,
                    UploadType::Message,
                    &temp_dir,
                    &requester_ip,
                )
            }
        })
        .await;

    match result {
        Ok(file_id) => {
            let upload_url = format!(
                "http://{}:{}/upload/{token}",
                config.server_addr, config.http_port
            );
            info!("New file upload: hash {hash}, file_id {file_id}");
            FileUploadResponse {
                msg_type: "file_upload_response".to_string(),
                status: ResponseStatus::New,
                upload_url: Some(upload_url),
                file_id: None,
                thumb_url: None,
                name: Some(name),
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to create temp upload: {e}");
            error_response(ResponseStatus::Error, "Database error")
        }
    }
}

fn error_response(status: ResponseStatus, message: impl Into<String>) -> FileUploadResponse {
    FileUploadResponse {
        msg_type: "file_upload_response".to_string(),
        status,
        upload_url: None,
        file_id: None,
        thumb_url: None,
        name: None,
        message: Some(message.into()),
    }
}
