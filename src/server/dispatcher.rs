use std::sync::Arc;

use futures_util::stream::SplitSink;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use super::connection::{send_response, WsStream};
use crate::auth::TokenMap;
use crate::config::Config;
use crate::db::{content_settings::ContentSettings, Database, DbError, DbService};
use crate::handlers;
use crate::model_detector::ModelDetector;
use crate::models::{ContentPart, ContentSettingsResponse, RequestPayload, ResponseStatus};
use crate::online::OnlineUsers;
use crate::rate_limiter::RateLimiter;

type Sender = Arc<Mutex<SplitSink<WsStream, Message>>>;

fn extract_field(payload: &RequestPayload, field_name: &str) -> Result<String, String> {
    let value = match field_name {
        "iccid" => payload.iccid.as_ref(),
        "username" => payload.username.as_ref(),
        "password" => payload.password.as_ref(),
        _ => return Err(format!("Unknown field: {field_name}")),
    };

    value
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| format!("Missing or empty field: {field_name}"))
}

async fn send_unauthorized(sender: &Sender, message: &str, debug: bool) -> Result<(), String> {
    let response = serde_json::json!({
        "type": "error",
        "status": "unauthorized",
        "message": message,
    });
    send_response(sender, &response, debug).await
}

fn require_auth(user: &Option<(String, String)>) -> Result<String, String> {
    user.as_ref()
        .map(|(u, _)| u.clone())
        .ok_or_else(|| "Not authenticated".to_string())
}

macro_rules! require_auth_or_return {
    ($sender:expr, $auth:expr, $debug:expr) => {
        match require_auth($auth) {
            Ok(u) => u,
            Err(e) => {
                send_unauthorized($sender, &e, $debug).await?;
                return Ok(());
            }
        }
    };
}

async fn recipient_of_message(
    db: &Arc<Database>,
    current_username: &str,
    msg_id: i64,
) -> Result<String, String> {
    let user_for_lookup = current_username.to_string();
    let other_id = db
        .call(move |d| {
            let uid = d.get_user_id_by_username(&user_for_lookup)?;
            let (s, r) = d.get_message_participants(msg_id)?;
            let other = if s == uid { r } else { s };
            Ok::<_, DbError>(other)
        })
        .await
        .map_err(|e| e.to_string())?;

    db.call(move |d| d.get_username_by_id(other_id))
        .await
        .map_err(|e| e.to_string())
}

async fn apply_content_filter(
    db: &Arc<Database>,
    model_detector: &Arc<tokio::sync::Mutex<ModelDetector>>,
    recipient: &str,
    content: &mut [ContentPart],
    config: &Config,
) -> Result<(), String> {
    if !config.model_enabled {
        return Ok(());
    }

    let recipient_for_lookup = recipient.to_string();
    let recipient_settings = match db
        .call(move |d| {
            let id = d.get_user_id_by_username(&recipient_for_lookup)?;
            d.get_content_settings(id)
        })
        .await
    {
        Ok(s) => s,
        Err(e) => {
            debug!("Skipping content filter; couldn't resolve recipient settings: {e}");
            ContentSettings::default()
        }
    };

    if !recipient_settings.any_enabled() {
        return Ok(());
    }

    let mut detector = model_detector.lock().await;
    for part in content.iter_mut() {
        if part.r#type != "text" {
            continue;
        }
        let Some(text) = part.text.as_deref() else {
            continue;
        };
        if text.is_empty() {
            continue;
        }

        let classification = detector
            .check_text(text)
            .map_err(|e| format!("Model detection error: {e}"))?;

        if recipient_settings.blocks_label(&classification.label) {
            debug!(
                "Blocked content part for recipient {recipient}: label={}, detected_by={:?}, confidence={:.3}",
                classification.label, classification.detected_by, classification.confidence
            );
            *part = ContentPart {
                r#type: "blocked_by_content_analysis".to_string(),
                r#class: Some(classification.label),
                text: None,
                file_id: None,
                hash: None,
                name: None,
                thumb_url: None,
                size: None,
                mark_down: None,
                target: None,
            };
        }
    }
    Ok(())
}

pub async fn handle_text_message(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    tokens: &TokenMap,
    rate_limiter: &Arc<RateLimiter>,
    model_detector: &Arc<tokio::sync::Mutex<ModelDetector>>,
    authenticated_username: &mut Option<(String, String)>,
    peer_addr: &str,
    text: &str,
    config: &Config,
) -> Result<(), String> {
    if !rate_limiter.check_and_consume(peer_addr).await {
        warn!("Rate limit exceeded for {peer_addr}");
        let response = serde_json::json!({
            "type": "error",
            "status": "rate_limited",
            "message": "Too many requests, please slow down"
        });
        send_response(sender, &response, config.debug).await?;
        return Ok(());
    }

    let payload: RequestPayload =
        serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;

    match payload.msg_type.as_str() {
        "signup" => {
            handle_signup(
                sender,
                db,
                tokens,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "signin" => {
            handle_signin(
                sender,
                db,
                tokens,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "search_user" => {
            handle_search_user(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "get_conversations" => {
            handle_get_conversations(sender, db, online_users, authenticated_username, config)
                .await?
        }
        "send_message" => {
            handle_send_message(
                sender,
                db,
                online_users,
                model_detector,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "get_chunk" => {
            handle_get_chunk(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "delete_message" => {
            handle_delete_message(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "edit_message" => {
            handle_edit_message(
                sender,
                db,
                online_users,
                model_detector,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "mark_read" => {
            handle_mark_read(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "logout" => handle_logout(sender, tokens, authenticated_username, config).await?,
        "file_upload_request" => {
            handle_file_upload_request(
                sender,
                db,
                authenticated_username,
                peer_addr,
                &payload,
                config,
            )
            .await?
        }
        "pin_message" => {
            handle_pin_message(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "get_pinned_messages" => {
            handle_get_pinned_messages(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "add_profile_picture" => {
            handle_add_profile_picture(
                sender,
                db,
                authenticated_username,
                peer_addr,
                &payload,
                config,
            )
            .await?
        }
        "remove_profile_picture" => {
            handle_remove_profile_picture(sender, db, authenticated_username, &payload, config)
                .await?
        }
        "get_profile_pictures" => {
            handle_get_profile_pictures(sender, db, authenticated_username, &payload, config)
                .await?
        }
        "set_primary_profile_picture" => {
            handle_set_primary_profile_picture(sender, db, authenticated_username, &payload, config)
                .await?
        }
        "unpin_message" => {
            handle_unpin_message(
                sender,
                db,
                online_users,
                authenticated_username,
                &payload,
                config,
            )
            .await?
        }
        "enable_spam_check" => {
            handle_set_check(
                sender,
                db,
                authenticated_username,
                config,
                CheckCategory::Spam,
                true,
            )
            .await?
        }
        "enable_obscene_check" => {
            handle_set_check(
                sender,
                db,
                authenticated_username,
                config,
                CheckCategory::Obscene,
                true,
            )
            .await?
        }
        "enable_hate_check" => {
            handle_set_check(
                sender,
                db,
                authenticated_username,
                config,
                CheckCategory::Hate,
                true,
            )
            .await?
        }
        "disable_spam_check" => {
            handle_set_check(
                sender,
                db,
                authenticated_username,
                config,
                CheckCategory::Spam,
                false,
            )
            .await?
        }
        "disable_obscene_check" => {
            handle_set_check(
                sender,
                db,
                authenticated_username,
                config,
                CheckCategory::Obscene,
                false,
            )
            .await?
        }
        "disable_hate_check" => {
            handle_set_check(
                sender,
                db,
                authenticated_username,
                config,
                CheckCategory::Hate,
                false,
            )
            .await?
        }
        "get_content_analysis_settings" => {
            handle_get_content_settings(sender, db, authenticated_username, config).await?
        }
        "add_conversation_lock" => {
            let user = require_auth_or_return!(sender, authenticated_username, config.debug);
            let with = payload
                .with
                .as_ref()
                .filter(|s| !s.is_empty())
                .ok_or("Missing 'with'")?
                .clone();
            let hashes = payload.hashes.clone().ok_or("Missing 'hashes'")?;

            let response = handlers::add_conversation_lock(
                Arc::clone(db),
                user,
                with,
                hashes,
                config,
                peer_addr,
            )
            .await;

            send_response(sender, &response, config.debug).await?;
        }
        "open_conversation_lock" => {
            let user = require_auth_or_return!(sender, authenticated_username, config.debug);
            let with = payload
                .with
                .as_ref()
                .filter(|s| !s.is_empty())
                .ok_or("Missing 'with'")?
                .clone();

            let hash = payload
                .hash
                .as_ref()
                .filter(|s| !s.is_empty())
                .ok_or("Missing 'hash'")?
                .clone();

            let response = handlers::open_conversation_lock(
                Arc::clone(db),
                user,
                with,
                hash,
                config,
                peer_addr,
            )
            .await;

            send_response(sender, &response, config.debug).await?;
        }
        unknown => {
            warn!("Unknown message type: {unknown}");
            let response = serde_json::json!({
                "type": "error",
                "status": "unknown_message_type",
                "message": format!("Unknown message type: {unknown}")
            });
            send_response(sender, &response, config.debug).await?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_signup(
    sender: &Sender,
    db: &Arc<Database>,
    tokens: &TokenMap,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    authenticated_username: &mut Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let iccid = extract_field(payload, "iccid")?;
    let username = extract_field(payload, "username")?;
    let password = extract_field(payload, "password")?;

    let response = handlers::handle_signup(
        Arc::clone(db),
        tokens.clone(),
        iccid,
        username.clone(),
        password,
        config,
    )
    .await;

    if response.status == ResponseStatus::Success {
        online_users
            .write()
            .await
            .add_sender(username.clone(), Arc::clone(sender));
        if let Some(token) = response.token.clone() {
            *authenticated_username = Some((username.clone(), token));
        }
        info!("User {username} signed up and is now online (this device)");
    }

    send_response(sender, &response, config.debug).await
}

async fn handle_signin(
    sender: &Sender,
    db: &Arc<Database>,
    tokens: &TokenMap,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    authenticated_username: &mut Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let username = extract_field(payload, "username")?;
    let password = extract_field(payload, "password")?;

    let response =
        handlers::handle_login(Arc::clone(db), tokens.clone(), username.clone(), password).await;

    if response.status == ResponseStatus::Success {
        online_users
            .write()
            .await
            .add_sender(username.clone(), Arc::clone(sender));
        if let Some(token) = response.token.clone() {
            *authenticated_username = Some((username.clone(), token));
        }
        info!("User {username} logged in and is now online (this device)");
    }

    send_response(sender, &response, config.debug).await
}

async fn handle_search_user(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let current_user = require_auth_or_return!(sender, auth, config.debug);
    let username = extract_field(payload, "username")?;

    let response = handlers::handle_search_user(
        Arc::clone(db),
        Arc::clone(online_users),
        current_user,
        username,
        config.chunk_size,
    )
    .await;

    send_response(sender, &response, config.debug).await
}

async fn handle_get_conversations(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let response = handlers::handle_get_conversations(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        config.chunk_size,
    )
    .await;
    send_response(sender, &response, config.debug).await
}

async fn handle_send_message(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    model_detector: &Arc<tokio::sync::Mutex<ModelDetector>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);

    let recipient = payload
        .recipient
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing recipient")?
        .clone();
    let mut content = payload
        .content
        .as_ref()
        .filter(|c| !c.is_empty())
        .ok_or("Missing content")?
        .clone();

    if let Err(e) = apply_content_filter(db, model_detector, &recipient, &mut content, config).await
    {
        error!("send_message filter error: {e}");
        let response = serde_json::json!({
            "type": "error",
            "status": "model_error",
            "message": "Content filter unavailable",
        });
        send_response(sender, &response, config.debug).await?;
        return Ok(());
    }

    let response = handlers::handle_send_message(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        recipient,
        content,
        config.chunk_size,
        config.debug,
    )
    .await;
    send_response(sender, &response, config.debug).await
}

#[derive(Clone, Copy)]
enum CheckCategory {
    Spam,
    Obscene,
    Hate,
}

impl CheckCategory {
    fn key(self) -> &'static str {
        match self {
            CheckCategory::Spam => "spam",
            CheckCategory::Obscene => "obscene",
            CheckCategory::Hate => "hate",
        }
    }

    fn apply(self, settings: &mut ContentSettings, enable: bool) {
        match self {
            CheckCategory::Spam => settings.spam = enable,
            CheckCategory::Obscene => settings.obscene = enable,
            CheckCategory::Hate => settings.hate = enable,
        }
    }
}

async fn handle_set_check(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    config: &Config,
    category: CheckCategory,
    enable: bool,
) -> Result<(), String> {
    let username = require_auth_or_return!(sender, auth, config.debug);

    let user_id = match {
        let u = username.clone();
        db.call(move |d| d.get_user_id_by_username(&u)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{username}': {e}");
            let response = serde_json::json!({
                "type": "error",
                "status": "error",
                "message": "User not found",
            });
            send_response(sender, &response, config.debug).await?;
            return Ok(());
        }
    };

    let mut settings = match db.call(move |d| d.get_content_settings(user_id)).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to get content settings for {username}: {e}");
            let response = serde_json::json!({
                "type": "error",
                "status": "error",
                "message": "Database error",
            });
            send_response(sender, &response, config.debug).await?;
            return Ok(());
        }
    };

    category.apply(&mut settings, enable);

    if let Err(e) = db
        .call(move |d| d.set_content_settings(user_id, settings))
        .await
    {
        error!("Failed to set content settings for {username}: {e}");
        let response = serde_json::json!({
            "type": "error",
            "status": "error",
            "message": "Database error",
        });
        send_response(sender, &response, config.debug).await?;
        return Ok(());
    }

    let action = if enable { "enable" } else { "disable" };
    let response_type = format!("{action}_{}_check_response", category.key());
    let response = serde_json::json!({
        "type": response_type,
        "status": "success",
    });
    send_response(sender, &response, config.debug).await
}

async fn handle_get_content_settings(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    config: &Config,
) -> Result<(), String> {
    let username = require_auth_or_return!(sender, auth, config.debug);

    let user_id = match {
        let u = username.clone();
        db.call(move |d| d.get_user_id_by_username(&u)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to get user ID for '{username}': {e}");
            let response = serde_json::json!({
                "type": "error",
                "status": "error",
                "message": "User not found",
            });
            send_response(sender, &response, config.debug).await?;
            return Ok(());
        }
    };

    let settings = match db.call(move |d| d.get_content_settings(user_id)).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to get content settings for {username}: {e}");
            let response = serde_json::json!({
                "type": "error",
                "status": "error",
                "message": "Database error",
            });
            send_response(sender, &response, config.debug).await?;
            return Ok(());
        }
    };

    let response = ContentSettingsResponse {
        msg_type: "get_content_analysis_settings_response".to_string(),
        spam: settings.spam,
        obscene: settings.obscene,
        hate: settings.hate,
    };
    send_response(sender, &response, config.debug).await
}

async fn handle_delete_message(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let msg_id = payload.id.ok_or("Missing id")?;
    let response = handlers::handle_delete_message(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        msg_id,
        config.debug,
    )
    .await;
    send_response(sender, &response, config.debug).await
}
async fn handle_get_chunk(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let with_user = payload
        .with
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing 'with' field")?
        .clone();
    let chunk_spec = payload.chunk.clone().ok_or("Missing 'chunk' field")?;

    let response = handlers::handle_get_messages_chunk(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        with_user,
        chunk_spec,
        config.chunk_size,
    )
    .await;
    send_response(sender, &response, config.debug).await
}
async fn handle_edit_message(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    model_detector: &Arc<tokio::sync::Mutex<ModelDetector>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let msg_id = payload.id.ok_or("Missing 'id' field")?;
    let mut content = payload
        .content
        .as_ref()
        .filter(|c| !c.is_empty())
        .ok_or("Missing 'content'")?
        .clone();

    if let Ok(recipient_username) = recipient_of_message(db, &user, msg_id).await {
        if let Err(e) = apply_content_filter(
            db,
            model_detector,
            &recipient_username,
            &mut content,
            config,
        )
        .await
        {
            error!("edit_message filter error: {e}");
            let response = serde_json::json!({
                "type": "error",
                "status": "model_error",
                "message": "Content filter unavailable",
            });
            send_response(sender, &response, config.debug).await?;
            return Ok(());
        }
    }

    let response = handlers::handle_edit_message(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        msg_id,
        content,
        config.debug,
    )
    .await;
    send_response(sender, &response, config.debug).await
}

async fn handle_mark_read(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let with_user = payload
        .with
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing 'with' field")?
        .clone();

    let response = handlers::handle_mark_read(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        with_user,
        payload.id,
        config.debug,
    )
    .await;
    send_response(sender, &response, config.debug).await
}

async fn handle_logout(
    sender: &Sender,
    tokens: &TokenMap,
    authenticated_username: &mut Option<(String, String)>,
    config: &Config,
) -> Result<(), String> {
    let user = match require_auth(authenticated_username) {
        Ok(u) => u,
        Err(e) => {
            send_unauthorized(sender, &e, config.debug).await?;
            return Ok(());
        }
    };

    let Some((_, token)) = authenticated_username.take() else {
        let response = serde_json::json!({
            "type": "error",
            "status": "not_authenticated",
            "message": "Not logged in"
        });
        send_response(sender, &response, config.debug).await?;
        return Ok(());
    };

    crate::auth::invalidate_token(tokens, &token).await;
    info!("User {user} logged out via explicit logout");

    let response = serde_json::json!({
        "type": "logout_response",
        "status": "success",
    });
    send_response(sender, &response, config.debug).await
}

async fn handle_file_upload_request(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    peer_addr: &str,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);

    let hash = payload
        .hash
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing hash")?
        .clone();
    let name = payload
        .name
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing name")?
        .clone();
    let size = payload.size.ok_or("Missing size")?;

    let response = handlers::handle_file_upload_request(
        Arc::clone(db),
        user,
        hash,
        name,
        size,
        config,
        peer_addr,
    )
    .await;
    send_response(sender, &response, config.debug).await
}

async fn handle_pin_message(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let message_id = payload.id.ok_or("Missing id")?;

    let response = handlers::handle_pin_message(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        message_id,
        config.debug,
    )
    .await;

    send_response(sender, &response, config.debug).await
}

async fn handle_get_pinned_messages(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);

    let chat_id = payload
        .with
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing 'with' field")?
        .clone();

    let response = handlers::handle_get_pinned_messages(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        chat_id,
        config.chunk_size,
    )
    .await;

    send_response(sender, &response, config.debug).await
}

async fn handle_add_profile_picture(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    peer_addr: &str,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let hash = payload.hash.clone().ok_or("Missing hash")?;
    let name = payload.name.clone().ok_or("Missing name")?;
    let size = payload.size.ok_or("Missing size")?;

    let response = handlers::handle_add_profile_picture(
        Arc::clone(db),
        user,
        hash,
        name,
        size,
        config,
        peer_addr,
    )
    .await;
    send_response(sender, &response, config.debug).await
}

async fn handle_remove_profile_picture(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let picture_id = payload.id.ok_or("Missing picture_id")?;
    let response = handlers::handle_remove_profile_picture(Arc::clone(db), user, picture_id).await;
    send_response(sender, &response, config.debug).await
}

async fn handle_get_profile_pictures(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let target_username = match payload.username.as_ref().filter(|s| !s.is_empty()) {
        Some(name) => name.clone(),
        None => {
            let response = serde_json::json!({
                "type": "error",
                "status": "bad_request",
                "message": "Missing 'username' field",
            });
            send_response(sender, &response, config.debug).await?;
            return Ok(());
        }
    };
    let response =
        handlers::handle_get_profile_pictures(Arc::clone(db), user, target_username).await;
    send_response(sender, &response, config.debug).await
}

async fn handle_set_primary_profile_picture(
    sender: &Sender,
    db: &Arc<Database>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let picture_id = payload.id.ok_or("Missing picture_id")?;
    let response =
        handlers::handle_set_primary_profile_picture(Arc::clone(db), user, picture_id).await;
    send_response(sender, &response, config.debug).await
}

async fn handle_unpin_message(
    sender: &Sender,
    db: &Arc<Database>,
    online_users: &Arc<tokio::sync::RwLock<OnlineUsers>>,
    auth: &Option<(String, String)>,
    payload: &RequestPayload,
    config: &Config,
) -> Result<(), String> {
    let user = require_auth_or_return!(sender, auth, config.debug);
    let message_id = payload.id.ok_or("Missing id")?;

    let response = handlers::handle_unpin_message(
        Arc::clone(db),
        Arc::clone(online_users),
        user,
        message_id,
        config.debug,
    )
    .await;

    send_response(sender, &response, config.debug).await
}
