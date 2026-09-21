use std::sync::Arc;

use chrono::{Duration, Local};
use rand::Rng;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::{Database, DbError, DbService};
use crate::models::{
    ProfilePictureAddResponse, ProfilePictureGetResponse, ProfilePictureInfo,
    ProfilePictureRemoveResponse, ProfilePictureSetPrimaryResponse, ResponseStatus, UploadType,
};
use crate::validators::validate_file_hash;

pub async fn add_profile_picture(
    db: Arc<Database>,
    username: String,
    hash: String,
    name: String,
    size: u64,
    config: &Config,
    requester_ip: &str,
) -> ProfilePictureAddResponse {
    if size > config.file_max_size_bytes {
        warn!("Profile picture upload denied: size {size} exceeds limit");
        return err_add(ResponseStatus::PayloadTooLarge, "File too large");
    }

    if let Err(e) = validate_file_hash(&hash) {
        warn!("Invalid hash format: {e}");
        return err_add(ResponseStatus::InvalidInput, e);
    }

    let user_id = match {
        let username = username.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{username}': {e}");
            return err_add(ResponseStatus::Error, "User not found");
        }
    };

    let existing = db
        .call({
            let hash = hash.clone();
            move |d| d.get_file_by_hash(&hash)
        })
        .await;

    match existing {
        Ok(Some(meta)) if !meta.is_temp => {
            let file_id = meta.id;
            match db
                .call(move |d| d.add_profile_picture(user_id, file_id))
                .await
            {
                Ok(()) => {
                    info!(
                        "Profile picture duplicate: hash {hash}, file_id {file_id} registered for {username}"
                    );
                    ProfilePictureAddResponse {
                        msg_type: "add_profile_picture_response".to_string(),
                        status: ResponseStatus::Duplicate,
                        upload_url: None,
                        file_id: Some(file_id),
                        name: Some(name),
                        message: None,
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to register duplicate profile picture {file_id} for {username}: {e}"
                    );
                    err_add(ResponseStatus::Error, "Database error")
                }
            }
        }

        Ok(Some(_)) => {
            warn!("Profile picture upload pending: hash {hash} already in progress");
            ProfilePictureAddResponse {
                msg_type: "add_profile_picture_response".to_string(),
                status: ResponseStatus::Pending,
                upload_url: None,
                file_id: None,
                name: None,
                message: Some("Upload already in progress".into()),
            }
        }
        Ok(None) => {
            create_profile_upload(db, user_id, hash, name, size, config, requester_ip).await
        }
        Err(e) => {
            error!("Error checking hash: {e}");
            err_add(ResponseStatus::Error, "Database error")
        }
    }
}

async fn create_profile_upload(
    db: Arc<Database>,
    user_id: i64,
    hash: String,
    name: String,
    size: u64,
    config: &Config,
    requester_ip: &str,
) -> ProfilePictureAddResponse {
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
                    Some(user_id),
                    UploadType::Profile,
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
            info!("New profile picture upload: hash {hash}, file_id {file_id}");
            ProfilePictureAddResponse {
                msg_type: "add_profile_picture_response".to_string(),
                status: ResponseStatus::New,
                upload_url: Some(upload_url),
                file_id: Some(file_id),
                name: Some(name),
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to create temp upload: {e}");
            err_add(ResponseStatus::Error, "Database error")
        }
    }
}

pub async fn remove_profile_picture(
    db: Arc<Database>,
    username: String,
    picture_id: i64,
) -> ProfilePictureRemoveResponse {
    let user_id = match {
        let username = username.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{username}': {e}");
            return ProfilePictureRemoveResponse {
                msg_type: "remove_profile_picture_response".to_string(),
                status: ResponseStatus::Error,
                message: Some("User not found".into()),
            };
        }
    };

    let result: Result<(), DbError> = db
        .call(move |d| d.remove_profile_picture(user_id, picture_id))
        .await;

    match result {
        Ok(()) => {
            info!("User {username} removed profile picture {picture_id}");
            ProfilePictureRemoveResponse {
                msg_type: "remove_profile_picture_response".to_string(),
                status: ResponseStatus::Success,
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to remove profile picture: {e}");
            ProfilePictureRemoveResponse {
                msg_type: "remove_profile_picture_response".to_string(),
                status: ResponseStatus::Error,
                message: Some(e.to_string()),
            }
        }
    }
}

pub async fn get_profile_pictures(
    db: Arc<Database>,
    _caller: String,
    target_username: String,
) -> ProfilePictureGetResponse {
    let target_username = target_username.to_lowercase();

    let target_user_id = match {
        let name = target_username.clone();
        db.call(move |d| d.get_user_id_by_username(&name)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to resolve user '{target_username}': {e}");
            return err_get("User not found");
        }
    };

    let result = db
        .call(move |d| d.get_profile_pictures(target_user_id))
        .await;

    match result {
        Ok(pics) => {
            let info_vec: Vec<ProfilePictureInfo> = pics
                .into_iter()
                .map(|p| ProfilePictureInfo {
                    url: p.url,
                    is_primary: p.is_primary,
                    uploaded_at: p.created_at,
                })
                .collect();
            info!(
                "Retrieved {} profile pictures for user {target_username}",
                info_vec.len()
            );
            ProfilePictureGetResponse {
                msg_type: "get_profile_pictures_response".to_string(),
                username: target_username,
                status: ResponseStatus::Success,
                pictures: Some(info_vec),
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to get profile pictures for user {target_username}: {e}");
            ProfilePictureGetResponse {
                msg_type: "get_profile_pictures_response".to_string(),
                username: target_username,
                status: ResponseStatus::Error,
                pictures: None,
                message: Some("Database error".into()),
            }
        }
    }
}

pub async fn set_primary_profile_picture(
    db: Arc<Database>,
    username: String,
    picture_id: i64,
) -> ProfilePictureSetPrimaryResponse {
    let user_id = match {
        let username = username.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{username}': {e}");
            return ProfilePictureSetPrimaryResponse {
                msg_type: "set_primary_profile_picture_response".to_string(),
                status: ResponseStatus::Error,
                message: Some("User not found".into()),
            };
        }
    };

    let result: Result<(), DbError> = db
        .call(move |d| d.set_primary_profile_picture(user_id, picture_id))
        .await;

    match result {
        Ok(()) => {
            info!("User {username} set primary profile picture to {picture_id}");
            ProfilePictureSetPrimaryResponse {
                msg_type: "set_primary_profile_picture_response".to_string(),
                status: ResponseStatus::Success,
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to set primary profile picture: {e}");
            ProfilePictureSetPrimaryResponse {
                msg_type: "set_primary_profile_picture_response".to_string(),
                status: ResponseStatus::Error,
                message: Some(e.to_string()),
            }
        }
    }
}

fn err_add(status: ResponseStatus, message: impl Into<String>) -> ProfilePictureAddResponse {
    ProfilePictureAddResponse {
        msg_type: "add_profile_picture_response".to_string(),
        status,
        upload_url: None,
        file_id: None,
        name: None,
        message: Some(message.into()),
    }
}

fn err_get(message: &str) -> ProfilePictureGetResponse {
    ProfilePictureGetResponse {
        msg_type: "get_profile_pictures_response".to_string(),
        username: String::new(),
        status: ResponseStatus::Error,
        pictures: None,
        message: Some(message.into()),
    }
}
