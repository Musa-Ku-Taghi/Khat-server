use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::db::{Database, DbError, DbService, FileMetadata};
use crate::models::{ContentPart, NewMessagePush, ResponseStatus, SendMessageResponse};
use crate::online::OnlineUsers;
use crate::validators::sanitize_mark_down;

pub async fn send_message(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    sender_username: String,
    recipient: String,
    content: Vec<ContentPart>,
    chunk_size: u64,
    debug: bool,
) -> SendMessageResponse {
    match crate::services::message::chat_is_locked(&db, &online_users, &sender_username, &recipient)
        .await
    {
        Ok(true) => {
            return SendMessageResponse {
                msg_type: "send_message_response".to_string(),
                status: ResponseStatus::LockedChatVerificationRequired,
                content: None,
                with_user: None,
                chunk_id: None,
                id: None,
                timestamp: None,
                message: Some(crate::services::message::LOCKED_MSG.to_string()),
            };
        }
        Ok(false) => {}
        Err(e) => error!("send_message gate check failed: {e}"),
    }

    let enriched_content = match enrich_content(&db, content).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let sender_id = match resolve_user_id(&db, &sender_username).await {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get sender ID for '{sender_username}': {e}");
            return err_response("Internal error");
        }
    };

    let recipient_id = match resolve_user_id(&db, &recipient).await {
        Ok(id) => id,
        Err(_) => {
            warn!("Recipient '{recipient}' not found");
            return err_response("Recipient not found");
        }
    };

    let msg_id = match db
        .call({
            let content = enriched_content.clone();
            move |d| d.save_message(sender_id, recipient_id, &content)
        })
        .await
    {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to save message: {e}");
            return err_response("Database error");
        }
    };

    let timestamp = match db.call(move |d| d.get_message_timestamp(msg_id)).await {
        Ok(ts) => ts,
        Err(e) => {
            error!("Failed to get message timestamp: {e}");
            return err_response("Database error");
        }
    };

    let chunk_id = match db
        .call({
            let message_id = msg_id;
            move |d| d.get_message_chunk_id(message_id, chunk_size)
        })
        .await
    {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get message chunk_id: {e}");
            return err_response("Database error");
        }
    };

    let recipient_sinks = {
        let online = online_users.read().await;
        online.get_senders(&recipient)
    };

    if recipient_sinks.is_empty() {
        info!("Recipient {recipient} is offline, message stored");
    } else {
        let mut outbound_content = enriched_content.clone();

        for part in &mut outbound_content {
            if part.r#type == "file" {
                part.size = None;
                part.hash = None;
            }
        }

        let push = NewMessagePush {
            msg_type: "new_message".to_string(),
            sender: sender_username.clone(),
            content: outbound_content,
            timestamp: timestamp.clone(),
            chunk_id,
            id: msg_id,
            edited_at: None,
        };

        for sink in &recipient_sinks {
            let sink = sink.clone();
            let push = push.clone();
            let recip = recipient.clone();

            tokio::spawn(async move {
                if let Err(e) = crate::server::send_response(&sink, &push, debug).await {
                    error!("Failed to push new_message to {recip}: {e}");
                }
            });
        }

        info!(
            "Pushed new_message to {} online devices of {recipient}",
            recipient_sinks.len()
        );
    }

    info!("Message from {sender_username} to {recipient} sent successfully");

    SendMessageResponse {
        msg_type: "send_message_response".to_string(),
        status: ResponseStatus::Success,
        content: Some(enriched_content),
        with_user: Some(recipient),
        chunk_id: Some(chunk_id),
        id: Some(msg_id),
        timestamp: Some(timestamp),
        message: None,
    }
}
async fn enrich_content(
    db: &Arc<Database>,
    content: Vec<ContentPart>,
) -> Result<Vec<ContentPart>, SendMessageResponse> {
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
                        warn!("Invalid markdown in message: {e}");
                        return Err(SendMessageResponse {
                            msg_type: "send_message_response".to_string(),
                            status: ResponseStatus::InvalidInput,
                            content: None,
                            with_user: None,
                            chunk_id: None,
                            id: None,
                            timestamp: None,
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
                    warn!("File part missing file_id");
                    return Err(err_response("Missing file_id"));
                }
            };

            let meta: Option<FileMetadata> = db
                .call(move |d| d.get_file_by_id(file_id))
                .await
                .map_err(|e| {
                    error!("DB error checking file {file_id}: {e}");
                    err_response("Database error")
                })?;

            let meta = match meta {
                Some(m) => m,
                None => {
                    warn!("File {file_id} not found");
                    return Err(err_response("File not found"));
                }
            };

            if meta.is_temp {
                warn!("Attempt to send message with temp file {file_id}");
                return Err(err_response("File not finalized"));
            }

            part.name = Some(meta.original_name);
            part.thumb_url = meta.thumb_path.map(|_| format!("/thumb/{}", meta.id));
            part.size = Some(meta.size as u64);
            part.hash = Some(meta.hash);
        }

        enriched.push(part);
    }

    Ok(enriched)
}

async fn resolve_user_id(db: &Arc<Database>, username: &str) -> Result<i64, DbError> {
    let username = username.to_string();
    db.call(move |d| d.get_user_id_by_username(&username)).await
}

async fn enrich_target_chunk_ids(db: &Arc<Database>, content: &mut [ContentPart], chunk_size: u64) {
    for part in content {
        if let Some(target) = &mut part.target {
            if let Ok(chunk_id) = db
                .call({
                    let message_id = target.message_id;
                    move |d| d.get_message_chunk_id(message_id, chunk_size)
                })
                .await
            {
                target.chunk_id = Some(chunk_id);
            }
        }
    }
}

fn err_response(message: &str) -> SendMessageResponse {
    SendMessageResponse {
        msg_type: "send_message_response".to_string(),
        status: ResponseStatus::Error,
        content: None,
        with_user: None,
        chunk_id: None,
        id: None,
        timestamp: None,
        message: Some(message.into()),
    }
}
