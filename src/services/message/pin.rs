use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::{Database, DbError, DbService};
use crate::models::{
    GetPinnedMessagesResponse, MessagePinnedPush, PinMessageResponse, PinnedMessage, ResponseStatus,
};
use crate::online::OnlineUsers;

pub async fn pin_message(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    message_id: i64,
    debug: bool,
) -> PinMessageResponse {
    let user_id = match {
        let username = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{current_user}': {e}");
            return PinMessageResponse {
                msg_type: "pin_message_response".to_string(),
                status: ResponseStatus::Error,
                message: Some("User not found".into()),
            };
        }
    };

    let (sender_id, recipient_id) = match db
        .call(move |d| d.get_message_participants(message_id))
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            error!("pin_message gate lookup failed: {e}");
            return PinMessageResponse {
                msg_type: "pin_message_response".to_string(),
                status: ResponseStatus::Error,
                message: Some("Message not found".into()),
            };
        }
    };
    let other_id = if sender_id == user_id {
        recipient_id
    } else {
        sender_id
    };
    let other_username = match db.call(move |d| d.get_username_by_id(other_id)).await {
        Ok(u) => u,
        Err(e) => {
            error!("pin_message gate lookup failed: {e}");
            return PinMessageResponse {
                msg_type: "pin_message_response".to_string(),
                status: ResponseStatus::Error,
                message: Some("User not found".into()),
            };
        }
    };
    match crate::services::message::chat_is_locked(
        &db,
        &online_users,
        &current_user,
        &other_username,
    )
    .await
    {
        Ok(true) => {
            return PinMessageResponse {
                msg_type: "pin_message_response".to_string(),
                status: ResponseStatus::LockedChatVerificationRequired,
                message: Some(crate::services::message::LOCKED_MSG.to_string()),
            };
        }
        Ok(false) => {}
        Err(e) => error!("pin_message gate check failed: {e}"),
    }

    let result: Result<(), DbError> = db.call(move |d| d.pin_message(user_id, message_id)).await;

    match result {
        Ok(()) => {
            let (sender_id, recipient_id) = match db
                .call(move |d| d.get_message_participants(message_id))
                .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    error!("Failed to get participants for pinned message {message_id}: {e}");
                    return PinMessageResponse {
                        msg_type: "pin_message_response".to_string(),
                        status: ResponseStatus::Success,
                        message: None,
                    };
                }
            };

            let other_id = if sender_id == user_id {
                recipient_id
            } else {
                sender_id
            };

            let other_username = match db.call(move |d| d.get_username_by_id(other_id)).await {
                Ok(username) => username,
                Err(e) => {
                    error!("Failed to get other username for pinned message {message_id}: {e}");
                    return PinMessageResponse {
                        msg_type: "pin_message_response".to_string(),
                        status: ResponseStatus::Success,
                        message: None,
                    };
                }
            };

            let content = match db.call(move |d| d.get_message_content(message_id)).await {
                Ok(content) => content,
                Err(e) => {
                    error!("Failed to get content for pinned message {message_id}: {e}");
                    return PinMessageResponse {
                        msg_type: "pin_message_response".to_string(),
                        status: ResponseStatus::Success,
                        message: None,
                    };
                }
            };

            let push_to_current = MessagePinnedPush {
                msg_type: "message_pinned".to_string(),
                with: other_username.clone(),
                content: content.clone(),
            };

            let push_to_other = MessagePinnedPush {
                msg_type: "message_pinned".to_string(),
                with: current_user.clone(),
                content,
            };

            let current_sinks = {
                let online = online_users.read().await;
                online.get_senders(&current_user)
            };

            let other_sinks = {
                let online = online_users.read().await;
                online.get_senders(&other_username)
            };

            for sink in current_sinks {
                let push = push_to_current.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::server::send_response(&sink, &push, debug).await {
                        error!("Failed to push message_pinned: {e}");
                    }
                });
            }

            for sink in other_sinks {
                let push = push_to_other.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::server::send_response(&sink, &push, debug).await {
                        error!("Failed to push message_pinned: {e}");
                    }
                });
            }

            info!("Message {message_id} pinned by {current_user}");

            PinMessageResponse {
                msg_type: "pin_message_response".to_string(),
                status: ResponseStatus::Success,
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to pin message {message_id}: {e}");
            PinMessageResponse {
                msg_type: "pin_message_response".to_string(),
                status: ResponseStatus::Error,
                message: Some(e.to_string()),
            }
        }
    }
}

pub async fn unpin_message(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    message_id: i64,
    debug: bool,
) -> PinMessageResponse {
    let user_id = match {
        let username = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{current_user}': {e}");
            return PinMessageResponse {
                msg_type: "unpin_message_response".to_string(),
                status: ResponseStatus::Error,
                message: Some("User not found".into()),
            };
        }
    };

    let result: Result<(), DbError> = db.call(move |d| d.unpin_message(user_id, message_id)).await;

    match result {
        Ok(()) => {
            let (sender_id, recipient_id) = match db
                .call(move |d| d.get_message_participants(message_id))
                .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    error!("Failed to get participants for unpinned message {message_id}: {e}");
                    return PinMessageResponse {
                        msg_type: "unpin_message_response".to_string(),
                        status: ResponseStatus::Success,
                        message: None,
                    };
                }
            };

            let other_id = if sender_id == user_id {
                recipient_id
            } else {
                sender_id
            };

            let other_username = match db.call(move |d| d.get_username_by_id(other_id)).await {
                Ok(username) => username,
                Err(e) => {
                    error!("Failed to get other username for unpinned message {message_id}: {e}");
                    return PinMessageResponse {
                        msg_type: "unpin_message_response".to_string(),
                        status: ResponseStatus::Success,
                        message: None,
                    };
                }
            };

            let push_to_current = serde_json::json!({
                "type": "message_unpinned",
                "with": other_username,
                "message_id": message_id
            });

            let push_to_other = serde_json::json!({
                "type": "message_unpinned",
                "with": current_user,
                "message_id": message_id
            });

            let current_sinks = {
                let online = online_users.read().await;
                online.get_senders(&current_user)
            };

            let other_sinks = {
                let online = online_users.read().await;
                online.get_senders(&other_username)
            };

            for sink in current_sinks {
                let push = push_to_current.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::server::send_response(&sink, &push, debug).await {
                        error!("Failed to push message_unpinned: {e}");
                    }
                });
            }

            for sink in other_sinks {
                let push = push_to_other.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::server::send_response(&sink, &push, debug).await {
                        error!("Failed to push message_unpinned: {e}");
                    }
                });
            }

            info!("Message {message_id} unpinned by {current_user}");

            PinMessageResponse {
                msg_type: "unpin_message_response".to_string(),
                status: ResponseStatus::Success,
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to unpin message {message_id}: {e}");
            PinMessageResponse {
                msg_type: "unpin_message_response".to_string(),
                status: ResponseStatus::Error,
                message: Some(e.to_string()),
            }
        }
    }
}

pub async fn get_pinned_messages(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    chat_id: String,
    chunk_size: u64,
) -> GetPinnedMessagesResponse {
    let user_id = match {
        let username = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&username)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{current_user}': {e}");
            return GetPinnedMessagesResponse {
                msg_type: "get_pinned_messages_response".to_string(),
                status: ResponseStatus::Error,
                content: None,
                message: Some("User not found".into()),
            };
        }
    };

    match crate::services::message::chat_is_locked(&db, &online_users, &current_user, &chat_id)
        .await
    {
        Ok(true) => {
            return GetPinnedMessagesResponse {
                msg_type: "get_pinned_messages_response".to_string(),
                status: ResponseStatus::LockedChatVerificationRequired,
                content: None,
                message: Some(crate::services::message::LOCKED_MSG.to_string()),
            };
        }
        Ok(false) => {}
        Err(e) => error!("get_pinned_messages gate check failed: {e}"),
    }

    let result = db
        .call({
            let chat_id = chat_id.clone();
            move |d| d.get_pinned_messages(user_id, &chat_id, chunk_size)
        })
        .await;

    match result {
        Ok(pins) => {
            let pinned: Vec<PinnedMessage> = pins
                .into_iter()
                .map(|(id, chunk_id, content)| PinnedMessage {
                    id,
                    chunk_id,
                    content,
                })
                .collect();

            info!(
                "Retrieved {} pinned messages for chat with {chat_id}",
                pinned.len()
            );

            GetPinnedMessagesResponse {
                msg_type: "get_pinned_messages_response".to_string(),
                status: ResponseStatus::Success,
                content: Some(pinned),
                message: None,
            }
        }
        Err(e) => {
            error!("Failed to get pinned messages for chat with {chat_id}: {e}");
            GetPinnedMessagesResponse {
                msg_type: "get_pinned_messages_response".to_string(),
                status: ResponseStatus::Error,
                content: None,
                message: Some("Database error".into()),
            }
        }
    }
}
