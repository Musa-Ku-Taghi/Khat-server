use std::collections::HashSet;
use std::sync::Arc;

use chrono::{Duration, Local};
use rand::Rng;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::config::Config;
use crate::db::{conversation_lock::SLOT_PROBE, Database, DbError, DbService};
use crate::models::{
    AddConversationLockResponse, OpenConversationLockResponse, ResponseStatus, UploadType,
};
use crate::online::OnlineUsers;

pub const LOCKED_MSG: &str = "Locked chat verification required";

pub async fn chat_is_locked(
    db: &Arc<Database>,
    online_users: &Arc<RwLock<OnlineUsers>>,
    owner_username: &str,
    partner_username: &str,
) -> Result<bool, DbError> {
    if online_users
        .read()
        .await
        .is_verified(owner_username, partner_username)
    {
        return Ok(false);
    }

    let owner = owner_username.to_string();
    let partner = partner_username.to_string();
    let (oid, pid) = db
        .call(move |d| {
            let a = d.get_user_id_by_username(&owner)?;
            let b = d.get_user_id_by_username(&partner)?;
            Ok::<_, DbError>((a, b))
        })
        .await?;

    let lock = db.call(move |d| d.get_conversation_lock(oid, pid)).await?;
    Ok(lock.is_some_and(|l| l.is_armed()))
}

pub async fn add_conversation_lock(
    db: Arc<Database>,
    current_user: String,
    partner_username: String,
    hashes: Vec<String>,
    config: &Config,
    requester_ip: &str,
) -> AddConversationLockResponse {
    if hashes.len() != 3 {
        return add_err(ResponseStatus::InvalidInput, "Expected exactly 3 hashes");
    }
    let hashes: [String; 3] = match hashes.try_into() {
        Ok(h) => h,
        Err(_) => return add_err(ResponseStatus::InvalidInput, "Expected exactly 3 hashes"),
    };

    let owner_id = match {
        let u = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&u)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("add_conversation_lock: owner lookup failed: {e}");
            return add_err(ResponseStatus::Error, "User not found");
        }
    };

    let partner_id = match {
        let p = partner_username.clone();
        db.call(move |d| d.get_user_id_by_username(&p)).await
    } {
        Ok(id) => id,
        Err(_) => return add_err(ResponseStatus::UsernameError, "Recipient not found"),
    };

    let existing = db
        .call(move |d| d.get_conversation_lock(owner_id, partner_id))
        .await;
    if let Ok(Some(l)) = existing {
        if l.hashes == hashes {
            info!("add_conversation_lock: duplicate for {current_user} -> {partner_username}");
            return AddConversationLockResponse {
                msg_type: "add_conversation_lock_response".to_string(),
                status: ResponseStatus::Duplicate,
                upload_urls: Vec::new(),
            };
        }
    }

    let create = db
        .call({
            let h = hashes.clone();
            move |d| d.create_conversation_lock(owner_id, partner_id, &h)
        })
        .await;
    if let Err(e) = create {
        error!("add_conversation_lock: create failed: {e}");
        return add_err(ResponseStatus::Error, "Database error");
    }
    let requester_ip = requester_ip.to_string();
    let mut upload_urls = Vec::with_capacity(3);
    for slot in 0..3_i32 {
        let token = gen_token(config.temp_token_bytes);
        let expires = Local::now().naive_local()
            + Duration::seconds(config.file_upload_temp_expiry_secs as i64);
        let temp_dir = config.temp_upload_dir.clone();
        let name = format!("face_lock_ref_{current_user}_{slot}");
        let hash = format!("facelock_{token}");

        let create_temp = db
            .call({
                let t = token.clone();
                let ip = requester_ip.clone();
                move |d| {
                    d.create_temp_upload(
                        &hash,
                        &name,
                        "image/jpeg",
                        10 * 1024 * 1024,
                        &t,
                        expires,
                        Some(owner_id),
                        UploadType::FaceLockRef,
                        &temp_dir,
                        &ip,
                    )
                }
            })
            .await;
        if let Err(e) = create_temp {
            error!("add_conversation_lock: temp create failed: {e}");
            return add_err(ResponseStatus::Error, "Database error");
        }

        let register = db
            .call({
                let t = token.clone();
                move |d| d.register_pending_face_upload(&t, owner_id, partner_id, slot)
            })
            .await;
        if let Err(e) = register {
            error!("add_conversation_lock: pending register failed: {e}");
            return add_err(ResponseStatus::Error, "Database error");
        }

        let host = format!("{}:{}", config.server_addr, config.http_port);
        upload_urls.push(format!(
            "{}://{}/upload/{}",
            config.upload_url_scheme, host, token
        ));
    }

    info!("add_conversation_lock: issued 3 URLs for {current_user} -> {partner_username}");
    AddConversationLockResponse {
        msg_type: "add_conversation_lock_response".to_string(),
        status: ResponseStatus::New,
        upload_urls,
    }
}

pub async fn open_conversation_lock(
    db: Arc<Database>,
    current_user: String,
    partner_username: String,
    _hash: String,
    config: &Config,
    requester_ip: &str,
) -> OpenConversationLockResponse {
    let owner_id = match {
        let u = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&u)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("open_conversation_lock: owner lookup failed: {e}");
            return open_err(ResponseStatus::Error);
        }
    };

    let partner_id = match {
        let p = partner_username.clone();
        db.call(move |d| d.get_user_id_by_username(&p)).await
    } {
        Ok(id) => id,
        Err(_) => return open_err(ResponseStatus::UsernameError),
    };

    let lock = db
        .call(move |d| d.get_conversation_lock(owner_id, partner_id))
        .await;
    let lock = match lock {
        Ok(Some(l)) => l,
        Ok(None) => return open_err(ResponseStatus::ChatNotLocked),
        Err(e) => {
            error!("open_conversation_lock: get failed: {e}");
            return open_err(ResponseStatus::Error);
        }
    };

    if !lock.is_armed() {
        return open_err(ResponseStatus::InvalidInput);
    }

    let token = gen_token(config.temp_token_bytes);
    let expires =
        Local::now().naive_local() + Duration::seconds(config.file_upload_temp_expiry_secs as i64);
    let temp_dir = config.temp_upload_dir.clone();
    let name = format!("face_lock_probe_{current_user}");
    let hash = format!("facelock_{token}");
    let requester_ip = requester_ip.to_string();
    let create_temp = db
        .call({
            let t = token.clone();
            let ip = requester_ip.clone();
            move |d| {
                d.create_temp_upload(
                    &hash,
                    &name,
                    "image/jpeg",
                    10 * 1024 * 1024,
                    &t,
                    expires,
                    Some(owner_id),
                    UploadType::FaceLockProbe,
                    &temp_dir,
                    &ip,
                )
            }
        })
        .await;
    if let Err(e) = create_temp {
        error!("open_conversation_lock: temp create failed: {e}");
        return open_err(ResponseStatus::Error);
    }

    let register = db
        .call({
            let t = token.clone();
            move |d| d.register_pending_face_upload(&t, owner_id, partner_id, SLOT_PROBE)
        })
        .await;
    if let Err(e) = register {
        error!("open_conversation_lock: pending register failed: {e}");
        return open_err(ResponseStatus::Error);
    }

    let host = format!("{}:{}", config.server_addr, config.http_port);
    let upload_url = format!("{}://{}/upload/{}", config.upload_url_scheme, host, token);

    info!("open_conversation_lock: issued probe URL for {current_user} -> {partner_username}");
    OpenConversationLockResponse {
        msg_type: "open_conversation_lock_response".to_string(),
        status: ResponseStatus::Ready,
        upload_url: Some(upload_url),
    }
}

pub async fn locked_partner_names(db: &Arc<Database>, owner_id: i64) -> HashSet<String> {
    let locks = match db.call(move |d| d.get_locks_owned_by(owner_id)).await {
        Ok(v) => v,
        Err(e) => {
            error!("locked_partner_names: get failed: {e}");
            return HashSet::new();
        }
    };

    let mut out = HashSet::new();
    for l in locks {
        if !l.is_armed() {
            continue;
        }
        let pid = l.partner_id;
        if let Ok(name) = db.call(move |d| d.get_username_by_id(pid)).await {
            out.insert(name);
        }
    }
    out
}

fn gen_token(nbytes: usize) -> String {
    let mut rng = rand::thread_rng();
    let mut bytes = vec![0u8; nbytes];
    rng.fill(&mut bytes[..]);
    hex::encode(bytes)
}

fn add_err(status: ResponseStatus, _msg: impl Into<String>) -> AddConversationLockResponse {
    AddConversationLockResponse {
        msg_type: "add_conversation_lock_response".to_string(),
        status,
        upload_urls: Vec::new(),
    }
}

fn open_err(status: ResponseStatus) -> OpenConversationLockResponse {
    OpenConversationLockResponse {
        msg_type: "open_conversation_lock_response".to_string(),
        status,
        upload_url: None,
    }
}
