use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::{Database, DbService};
use crate::models::{DeleteMessageResponse, ResponseStatus};
use crate::online::OnlineUsers;

pub async fn delete_message(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    message_id: i64,
    debug: bool,
) -> DeleteMessageResponse {
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

    let (sender_id, recipient_id) = match db
        .call(move |d| d.get_message_participants(message_id))
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            error!("Failed to get message info for deletion: {e}");
            return err("Message not found");
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
            error!("delete_message gate lookup failed: {e}");
            return err("Database error");
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
            return DeleteMessageResponse {
                msg_type: "delete_message_response".to_string(),
                status: ResponseStatus::LockedChatVerificationRequired,
                message: Some(crate::services::message::LOCKED_MSG.to_string()),
            };
        }
        Ok(false) => {}
        Err(e) => error!("delete_message gate check failed: {e}"),
    }

    if let Err(e) = db
        .call(move |d| d.delete_message(message_id, user_id))
        .await
    {
        error!("Failed to delete message {message_id}: {e}");
        return DeleteMessageResponse {
            msg_type: "delete_message_response".to_string(),
            status: ResponseStatus::Error,
            message: Some(e.to_string()),
        };
    }

    info!("Message {message_id} deleted by {current_user}");

    let other_id = if sender_id == user_id {
        recipient_id
    } else {
        sender_id
    };

    let other_username = db
        .call(move |d| d.get_username_by_id(other_id))
        .await
        .unwrap_or_else(|e| {
            error!("Failed to get other username for push: {e}");
            String::new()
        });

    if !other_username.is_empty() {
        push_deleted(online_users, &other_username, message_id, debug).await;
    }

    DeleteMessageResponse {
        msg_type: "delete_message_response".to_string(),
        status: ResponseStatus::Success,
        message: None,
    }
}

async fn push_deleted(
    online_users: Arc<RwLock<OnlineUsers>>,
    username: &str,
    message_id: i64,
    debug: bool,
) {
    let sinks = {
        let online = online_users.read().await;
        online.get_senders(username)
    };

    if sinks.is_empty() {
        info!("Other user {username} offline, no push for deletion");
        return;
    }

    let payload = serde_json::json!({
        "type": "message_deleted",
        "message_id": message_id,
    });

    for sink in sinks {
        let payload = payload.clone();
        let username = username.to_string();
        tokio::spawn(async move {
            if let Err(e) = crate::server::send_response(&sink, &payload, debug).await {
                error!("Failed to push message_deleted to {username}: {e}");
            }
        });
    }

    info!("Pushed message_deleted to {username} for message {message_id}");
}

fn err(message: &str) -> DeleteMessageResponse {
    DeleteMessageResponse {
        msg_type: "delete_message_response".to_string(),
        status: ResponseStatus::Error,
        message: Some(message.into()),
    }
}
