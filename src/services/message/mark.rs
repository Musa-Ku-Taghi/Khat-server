use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::{Database, DbError, DbService};
use crate::models::{MarkReadResponse, MessageReadPush, ResponseStatus};
use crate::online::OnlineUsers;

pub async fn mark_read(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    target: String,
    msg_id: Option<i64>,
    debug: bool,
) -> MarkReadResponse {
    let (current_id, target_id) = match {
        let current = current_user.clone();
        let target_clone = target.clone();
        db.call(move |d| {
            let a = d.get_user_id_by_username(&current)?;
            let b = d.get_user_id_by_username(&target_clone)?;
            Ok::<_, DbError>((a, b))
        })
        .await
    } {
        Ok(ids) => ids,
        Err(e) => {
            error!("Failed to resolve user IDs for mark_read: {e}");
            return err("User not found");
        }
    };

    match crate::services::message::chat_is_locked(&db, &online_users, &current_user, &target).await
    {
        Ok(true) => {
            return MarkReadResponse {
                msg_type: "mark_read_response".to_string(),
                status: ResponseStatus::LockedChatVerificationRequired,
                message: Some(crate::services::message::LOCKED_MSG.to_string()),
            };
        }
        Ok(false) => {}
        Err(e) => error!("mark_read gate check failed: {e}"),
    }

    let (affected_ids, sender_username): (Vec<i64>, Option<String>) = if let Some(id) = msg_id {
        match mark_single(&db, id, current_id).await {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to mark message as read: {e}");
                return err("Database error");
            }
        }
    } else {
        mark_all_from_sender(&db, current_id, target_id, &current_user, &target).await
    };

    if let Some(sender) = sender_username {
        if !affected_ids.is_empty() {
            push_read_receipts(online_users, &sender, affected_ids, &current_user, debug).await;
        }
    }

    MarkReadResponse {
        msg_type: "mark_read_response".to_string(),
        status: ResponseStatus::Success,
        message: None,
    }
}

async fn mark_single(
    db: &Arc<Database>,
    message_id: i64,
    current_id: i64,
) -> Result<(Vec<i64>, Option<String>), DbError> {
    match db
        .call(move |d| d.mark_message_read(message_id, current_id))
        .await?
    {
        Some((sender_id, msg_id)) => {
            let sender_username = db.call(move |d| d.get_username_by_id(sender_id)).await.ok();

            match sender_username {
                Some(username) => {
                    info!("Marked message {msg_id} as read");
                    Ok((vec![msg_id], Some(username)))
                }
                None => {
                    error!("Failed to get sender username for message {msg_id}");
                    Ok((vec![msg_id], None))
                }
            }
        }
        None => {
            info!("Message {message_id} already read or not found");
            Ok((vec![], None))
        }
    }
}

async fn mark_all_from_sender(
    db: &Arc<Database>,
    current_id: i64,
    target_id: i64,
    current_user: &str,
    target: &str,
) -> (Vec<i64>, Option<String>) {
    let result = db
        .call(move |d| d.mark_read_for_sender(current_id, target_id))
        .await;

    match result {
        Ok(ids) => {
            info!(
                "Marked {} messages from {} to {} as read",
                ids.len(),
                target,
                current_user
            );
            if ids.is_empty() {
                (vec![], None)
            } else {
                (ids, Some(target.to_string()))
            }
        }
        Err(e) => {
            error!("Failed to mark messages as read: {e}");
            (vec![], None)
        }
    }
}

async fn push_read_receipts(
    online_users: Arc<RwLock<OnlineUsers>>,
    sender_username: &str,
    message_ids: Vec<i64>,
    reader_username: &str,
    debug: bool,
) {
    if message_ids.is_empty() {
        return;
    }

    let sinks = {
        let online = online_users.read().await;
        online.get_senders(sender_username)
    };

    if sinks.is_empty() {
        info!(
            "Sender {} is offline, read receipts not pushed",
            sender_username
        );
        return;
    }

    for msg_id in message_ids {
        let push = MessageReadPush {
            msg_type: "message_read".to_string(),
            message_id: msg_id,
            reader: reader_username.to_string(),
        };

        for sink in &sinks {
            let sink = sink.clone();
            let push = push.clone();
            let sender = sender_username.to_string();

            tokio::spawn(async move {
                if let Err(e) = crate::server::send_response(&sink, &push, debug).await {
                    error!(
                        "Failed to push read receipt for message {} to {}: {}",
                        msg_id, sender, e
                    );
                }
            });
        }
    }
}

fn err(message: &str) -> MarkReadResponse {
    MarkReadResponse {
        msg_type: "mark_read_response".to_string(),
        status: ResponseStatus::Error,
        message: Some(message.into()),
    }
}
