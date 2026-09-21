use std::sync::Arc;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::auth::TokenMap;
use crate::config::Config;
use crate::db::Database;
use crate::model_detector::ModelDetector;
use crate::online::OnlineUsers;
use crate::rate_limiter::RateLimiter;

use super::dispatcher::handle_text_message;

pub type WsStream = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

type Sender = Arc<Mutex<SplitSink<WsStream, Message>>>;

pub async fn send_response<T: Serialize>(
    sender: &Mutex<SplitSink<WsStream, Message>>,
    response: &T,
    debug_enabled: bool,
) -> Result<(), String> {
    let text = serde_json::to_string(response)
        .map_err(|e| format!("Failed to serialize response: {e}"))?;
    if debug_enabled {
        debug!("SENDING: {text}");
    }
    let mut sender = sender.lock().await;
    sender
        .send(Message::Text(text))
        .await
        .map_err(|e| format!("Failed to send WebSocket message: {e}"))
}

async fn heartbeat_task(
    sender: Sender,
    last_pong: Arc<Mutex<Instant>>,
    ping_interval: Duration,
    pong_timeout: Duration,
    cancel_token: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("Heartbeat task cancelled");
                break;
            }
            _ = sleep(ping_interval) => {
                debug!("Sending ping");
                if sender.lock().await.send(Message::Ping(vec![])).await.is_err() {
                    warn!("Failed to send ping, connection likely closed");
                    break;
                }

                let ping_sent = Instant::now();
                sleep(pong_timeout).await;

                if *last_pong.lock().await < ping_sent {
                    warn!("Pong timeout, closing connection");
                    let _ = sender.lock().await.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

pub async fn handle_connection(
    ws: WsStream,
    db: Arc<Database>,
    online_users: Arc<tokio::sync::RwLock<OnlineUsers>>,
    config: Arc<Config>,
    tokens: TokenMap,
    rate_limiter: Arc<RateLimiter>,
    model_detector: Arc<tokio::sync::Mutex<ModelDetector>>,
    cancel_token: CancellationToken,
) {
    let peer_addr = ws
        .get_ref()
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    info!("New WebSocket connection from {peer_addr}");

    let (ws_sender, mut ws_receiver) = ws.split();
    let sender: Sender = Arc::new(Mutex::new(ws_sender));
    let last_pong = Arc::new(Mutex::new(Instant::now()));

    let heartbeat_handle = {
        let sender = Arc::clone(&sender);
        let last_pong = Arc::clone(&last_pong);
        let token = cancel_token.clone();
        let ping_interval = Duration::from_secs(config.ping_interval_secs);
        let pong_timeout = Duration::from_secs(config.pong_timeout_secs);
        tokio::spawn(async move {
            heartbeat_task(sender, last_pong, ping_interval, pong_timeout, token).await
        })
    };

    let mut authenticated_username: Option<(String, String)> = None;

    loop {
        tokio::select! {
            msg_result = ws_receiver.next() => {
                match msg_result {
                    Some(Ok(Message::Text(text))) => {
                        if config.debug {
                            debug!("RECEIVED: {text}");
                        }
                        if let Err(e) = handle_text_message(
                            &sender,
                            &db,
                            &online_users,
                            &tokens,
                            &rate_limiter,
                            &model_detector,
                            &mut authenticated_username,
                            &peer_addr,
                            &text,
                            &config,
                        )
                        .await
                        {
                            error!("Error handling message from {peer_addr}: {e}");
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("Connection closed by {peer_addr}");
                        break;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        *last_pong.lock().await = Instant::now();
                        debug!("Pong received");
                    }
                    Some(Ok(Message::Binary(_))) => {
                        warn!("Binary messages not supported from {peer_addr}");
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        error!("WebSocket error: {e}");
                        break;
                    }
                    None => {
                        info!("Connection closed by {peer_addr}");
                        break;
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                info!("Shutdown signalled, exiting connection loop");
                break;
            }
        }
    }

    if cancel_token.is_cancelled() {
        let _ = send_response(
            &sender,
            &serde_json::json!({"type": "shutdown", "reason": "server shutting down"}),
            config.debug,
        )
        .await;
        let _ = sender.lock().await.send(Message::Close(None)).await;
    }

    heartbeat_handle.abort();

    if let Some((username, token)) = authenticated_username {
        crate::auth::invalidate_token(&tokens, &token).await;
        info!("Token invalidated for user {username}");

        let mut online = online_users.write().await;
        let still_online = online.remove_sender(&username, &sender);
        if still_online {
            info!("One device of user {username} disconnected (still online from other devices)");
        } else {
            online.clear_verified(&username);
            info!(
                "User {username} went offline (no more active connections); cleared verified chat locks"
            );
        }
        drop(online);
    }

    info!("Connection terminated from {peer_addr}");
}
