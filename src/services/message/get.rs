use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::{Database, DbError, DbService};
use crate::models::{
    ChunkSpec, Conversation, GetConversationsResponse, GetMessagesResponse, Message, ResponseStatus,
};
use crate::online::OnlineUsers;
use crate::services::message::{chat_is_locked, locked_partner_names, LOCKED_MSG};

pub async fn get_messages_chunk(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    target: String,
    chunk_spec: ChunkSpec,
    chunk_size: u64,
) -> GetMessagesResponse {
    match chat_is_locked(&db, &online_users, &current_user, &target).await {
        Ok(true) => return locked_response(),
        Ok(false) => {}
        Err(e) => error!("get_chunk gate check failed: {e}"),
    }

    let (current_id, other_id) = {
        let current = current_user.clone();
        let target_clone = target.clone();
        match db
            .call(move |d| {
                let a = d.get_user_id_by_username(&current)?;
                let b = d.get_user_id_by_username(&target_clone)?;
                Ok::<_, DbError>((a, b))
            })
            .await
        {
            Ok(ids) => ids,
            Err(e) => {
                error!("Failed to resolve user IDs: {e}");
                return error_response("User not found");
            }
        }
    };

    let chunk = match chunk_spec {
        ChunkSpec::Number(n) => n,
        ChunkSpec::Unread => db
            .call(move |d| d.get_unread_offset_for_conversation(current_id, other_id, chunk_size))
            .await
            .ok()
            .flatten()
            .unwrap_or(0),
    };

    let offset = (chunk * chunk_size) as i64;
    let limit = chunk_size as i64;

    let messages: Result<Vec<Message>, DbError> = db
        .call(move |d| d.get_messages_between_paginated(current_id, other_id, limit, offset))
        .await;

    match messages {
        Ok(mut msgs) => {
            enrich_target_chunk_ids(&db, &mut msgs, chunk_size).await;
            info!(
                "Retrieved {} messages between {} and {} (chunk {}, size {})",
                msgs.len(),
                current_user,
                target,
                chunk,
                chunk_size
            );
            GetMessagesResponse {
                msg_type: "get_messages_response".to_string(),
                status: ResponseStatus::Success,
                messages: Some(msgs),
                message: None,
                chunk_id: Some(chunk),
            }
        }
        Err(e) => {
            error!("Failed to get messages: {e}");
            GetMessagesResponse {
                msg_type: "get_messages_response".to_string(),
                status: ResponseStatus::Error,
                messages: None,
                message: Some("Database error".into()),
                chunk_id: Some(chunk),
            }
        }
    }
}

fn locked_response() -> GetMessagesResponse {
    GetMessagesResponse {
        msg_type: "get_messages_response".to_string(),
        status: ResponseStatus::LockedChatVerificationRequired,
        messages: None,
        message: Some(LOCKED_MSG.to_string()),
        chunk_id: None,
    }
}

fn error_response(message: &str) -> GetMessagesResponse {
    GetMessagesResponse {
        msg_type: "get_messages_response".to_string(),
        status: ResponseStatus::Error,
        messages: None,
        message: Some(message.into()),
        chunk_id: None,
    }
}

pub async fn get_conversations(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    chunk_size: u64,
) -> GetConversationsResponse {
    let user_id = match {
        let current = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&current)).await
    } {
        Ok(id) => id,
        Err(_) => {
            error!("User '{current_user}' not found for conversations");
            return GetConversationsResponse {
                msg_type: "get_conversations_response".to_string(),
                status: ResponseStatus::Error,
                conversations: None,
                message: Some("User not found".into()),
            };
        }
    };

    let conversations: Result<Vec<Conversation>, DbError> = db
        .call(move |d| d.get_conversations_for_user(user_id, chunk_size))
        .await;

    match conversations {
        Ok(mut convs) => {
            enrich_conversation_files(&db, &mut convs).await;

            for conv in &mut convs {
                let with_user = conv.with_user.clone();

                let with_user_id = match db
                    .call({
                        let with_user = with_user.clone();
                        move |d| d.get_user_id_by_username(&with_user)
                    })
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        error!(
                        "Failed to resolve conversation partner '{with_user}' for chunk_count: {e}"
                    );
                        conv.chunk_count = 0;
                        continue;
                    }
                };

                conv.chunk_count = match db
                    .call({
                        let user_id = user_id;
                        move |d| d.get_conversation_chunk_count(user_id, with_user_id, chunk_size)
                    })
                    .await
                {
                    Ok(count) => count,
                    Err(e) => {
                        error!(
                            "Failed to get chunk_count for conversation with '{with_user}': {e}"
                        );
                        0
                    }
                };
            }

            let locked_partners = locked_partner_names(&db, user_id).await;

            {
                let online = online_users.read().await;

                for conv in &mut convs {
                    conv.online = online.is_online(&conv.with_user);

                    if locked_partners.contains(&conv.with_user)
                        && !online.is_verified(&current_user, &conv.with_user)
                    {
                        conv.locked = true;
                        conv.last_message = Vec::new();
                    }
                }
            }

            info!("Retrieved {} conversations for {current_user}", convs.len());

            GetConversationsResponse {
                msg_type: "get_conversations_response".to_string(),
                status: ResponseStatus::Success,
                conversations: Some(convs),
                message: None,
            }
        }

        Err(e) => {
            error!("Failed to get conversations for {current_user}: {e}");

            GetConversationsResponse {
                msg_type: "get_conversations_response".to_string(),
                status: ResponseStatus::Error,
                conversations: None,
                message: Some("Database error".to_string()),
            }
        }
    }
}

async fn enrich_conversation_files(db: &Arc<Database>, convs: &mut [Conversation]) {
    for conv in convs.iter_mut() {
        for part in conv.last_message.iter_mut() {
            if part.r#type != "file" {
                continue;
            }
            let Some(file_id) = part.file_id else {
                continue;
            };
            if let Ok(Some(meta)) = db.call(move |d| d.get_file_by_id(file_id)).await {
                part.size = Some(meta.size as u64);
                part.hash = Some(meta.hash);
            }
        }
    }
}

async fn enrich_target_chunk_ids(db: &Arc<Database>, messages: &mut [Message], chunk_size: u64) {
    for message in messages {
        for part in &mut message.content {
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
}
