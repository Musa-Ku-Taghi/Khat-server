use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::db::{Database, DbService, FileMetadata};
use crate::models::{ContentPart, EditMessageResponse, ResponseStatus};
use crate::online::OnlineUsers;
use crate::validators::sanitize_mark_down;

pub async fn edit_message(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    message_id: i64,
    new_content: Vec<ContentPart>,
    debug: bool,
) -> EditMessageResponse {
    let user_id = match {
        let username = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{current_user}': {e}");
            return err("User not found");
        }
    };

    let enriched_content = match enrich_content(&db, new_content).await {
        Ok(c) => c,
        Err(v) => return v,
    };

    let edited_at_str = match {
        let content = enriched_content.clone();
        db.call(move |d| d.edit_message(message_id, user_id, &content))
            .await
    } {
        Ok(ts) => ts,
        Err(e) => {
            let msg = e.to_string();
            error!("Failed to edit message {message_id}: {msg}");
            return err(&msg);
        }
    };

    let (sender_id, recipient_id) = match db
        .call(move |d| d.get_message_participants(message_id))
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            error!("Failed to get participants for push: {e}");
            return success_no_push();
        }
    };

    let other_id_for_gate = if sender_id == user_id {
        recipient_id
    } else {
        sender_id
    };
    let other_username_for_gate = match db
        .call(move |d| d.get_username_by_id(other_id_for_gate))
        .await
    {
        Ok(u) => u,
        Err(e) => {
            error!("edit_message gate lookup failed: {e}");
            return success_no_push();
        }
    };
    match crate::services::message::chat_is_locked(
        &db,
        &online_users,
        &current_user,
        &other_username_for_gate,
    )
    .await
    {
        Ok(true) => {
            return EditMessageResponse {
                msg_type: "edit_message_response".to_string(),
                status: ResponseStatus::LockedChatVerificationRequired,
                message: Some(crate::services::message::LOCKED_MSG.to_string()),
            };
        }
        Ok(false) => {}
        Err(e) => error!("edit_message gate check failed: {e}"),
    }

    let sender_username = match db.call(move |d| d.get_username_by_id(sender_id)).await {
        Ok(u) => u,
        Err(e) => {
            error!("Failed to get sender username: {e}");
            return success_no_push();
        }
    };
    let recipient_username = db
        .call(move |d| d.get_username_by_id(recipient_id))
        .await
        .unwrap_or_else(|e| {
            error!("Failed to get recipient username: {e}");
            String::new()
        });

    push_edit_to_participants(
        online_users,
        &sender_username,
        &recipient_username,
        message_id,
        enriched_content,
        edited_at_str,
        debug,
    )
    .await;

    success_no_push()
}

async fn enrich_content(
    db: &Arc<Database>,
    content: Vec<ContentPart>,
) -> Result<Vec<ContentPart>, EditMessageResponse> {
    let mut enriched = Vec::with_capacity(content.len());

    for mut part in content {
        if part.r#type == "text" {
            if let Some(md) = part.mark_down.take() {
                match sanitize_mark_down(md) {
                    Ok(cleaned) => {
                        part.mark_down = if cleaned.is_empty() {
                            None
                        } else {
                            Some(cleaned)
                        };
                    }
                    Err(e) => {
                        warn!("Invalid markdown in edit: {e}");
                        return Err(EditMessageResponse {
                            msg_type: "edit_message_response".to_string(),
                            status: ResponseStatus::InvalidInput,
                            message: Some(format!("Invalid markdown: {e}")),
                        });
                    }
                }
            }
        } else {
            part.mark_down = None;
        }

        if part.r#type == "file" {
            let file_id = match part.file_id {
                Some(id) => id,
                None => {
                    warn!("File part missing file_id in edit");
                    return Err(err("Missing file_id"));
                }
            };

            let meta: Option<FileMetadata> = db
                .call(move |d| d.get_file_by_id(file_id))
                .await
                .map_err(|e| {
                    error!("DB error checking file {file_id} during edit: {e}");
                    err("Database error")
                })?;

            let meta = match meta {
                Some(m) => m,
                None => {
                    warn!("File {file_id} not found during edit");
                    return Err(err("File not found"));
                }
            };

            if meta.is_temp {
                warn!("Attempt to edit message with temp file {file_id}");
                return Err(err("Cannot reference a temporary file"));
            }

            part.size = Some(meta.size as u64);
            part.thumb_url = meta.thumb_path.map(|_| format!("/thumb/{}", meta.id));
            if part.name.as_deref().map_or(true, |s| s.is_empty()) {
                part.name = Some(meta.original_name);
            }
            part.hash = Some(meta.hash);
        }

        enriched.push(part);
    }

    Ok(enriched)
}

async fn push_edit_to_participants(
    online_users: Arc<RwLock<OnlineUsers>>,
    sender_username: &str,
    recipient_username: &str,
    message_id: i64,
    content: Vec<ContentPart>,
    edited_at: String,
    debug: bool,
) {
    let payload = serde_json::json!({
        "type": "message_edited",
        "message_id": message_id,
        "content": content,
        "edited_at": edited_at,
    });

    let sinks = {
        let online = online_users.read().await;
        let mut sinks = online.get_senders(sender_username);
        if !recipient_username.is_empty() && recipient_username != sender_username {
            sinks.extend(online.get_senders(recipient_username));
        }
        sinks
    };

    if sinks.is_empty() {
        return;
    }

    for sink in sinks {
        let payload = payload.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::server::send_response(&sink, &payload, debug).await {
                error!("Failed to push message_edited: {e}");
            }
        });
    }
    info!("Pushed message_edited for message {message_id} to participants");
}

fn err(message: &str) -> EditMessageResponse {
    EditMessageResponse {
        msg_type: "edit_message_response".to_string(),
        status: ResponseStatus::Error,
        message: Some(message.to_string()),
    }
}

fn success_no_push() -> EditMessageResponse {
    EditMessageResponse {
        msg_type: "edit_message_response".to_string(),
        status: ResponseStatus::Success,
        message: None,
    }
}
