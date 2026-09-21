mod auth;
mod config;
mod db;
mod error;
mod face_detector;
mod file_cleanup;
mod file_server;
mod handlers;
mod model_detector;
mod models;
mod online;
mod rate_limiter;
mod server;
mod services;
mod utils;
mod validators;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::db::Database;
use crate::online::OnlineUsers;
use crate::rate_limiter::RateLimiter;
use crate::server::handle_connection;

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() {
    let config = Arc::new(Config::load_or_create());
    config.validate();

    let detector = model_detector::ModelDetector::new(&config.model_dir, config.model_enabled)
        .expect("Failed to initialize model detector");
    let model_detector = Arc::new(tokio::sync::Mutex::new(detector));

    let face_detector = Arc::new(tokio::sync::Mutex::new(
        face_detector::FaceDetector::new(&config.face_model_dir, config.face_model_enabled)
            .expect("Failed to initialize face detector"),
    ));

    let log_level = if config.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    if let Err(e) = tracing_subscriber::fmt()
        .with_max_level(log_level)
        .try_init()
    {
        eprintln!("Warning: Could not set tracing subscriber: {}", e);
    }

    let db = Arc::new(
        Database::new(&config.db_path, Arc::clone(&config)).expect("Failed to initialize database"),
    );
    let online_users = Arc::new(RwLock::new(OnlineUsers::new()));
    let tokens = auth::TokenMap::default();

    let rate_limiter = Arc::new(RateLimiter::new(
        config.rate_limit_enabled,
        config.rate_limit_requests_per_minute,
    ));

    if config.rate_limit_enabled {
        let limiter = rate_limiter.clone();
        let cleanup_interval = config.rate_limit_cleanup_interval_secs;
        tokio::spawn(async move {
            rate_limiter::rate_limiter_cleanup_task(limiter, cleanup_interval).await;
        });
    }

    let cancel_token = CancellationToken::new();
    let child_token = cancel_token.clone();
    let http_cancel_token = cancel_token.clone();

    let http_addr: SocketAddr = format!("{}:{}", config.server_addr, config.http_port)
        .parse()
        .expect("Invalid HTTP address");

    let app_state = Arc::new(file_server::AppState {
        db: db.clone(),
        config: config.clone(),
        tokens: tokens.clone(),
        online_users: online_users.clone(),
        face_detector: face_detector.clone(),
    });
    let app = file_server::create_router(app_state);

    let http_server = tokio::spawn(async move {
        let listener = TcpListener::bind(&http_addr)
            .await
            .expect("Failed to bind HTTP");
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown({
            let token = http_cancel_token.clone();
            async move {
                token.cancelled().await;
                tracing::info!("HTTP server graceful shutdown initiated");
            }
        })
        .await
        .expect("HTTP server failed");
    });
    tracing::info!(
        "HTTP server running on http://{}:{}",
        config.server_addr,
        config.http_port
    );

    let cleanup_interval = config.cleanup_interval_secs;
    let cleanup_db = db.clone();
    tokio::spawn(async move {
        file_cleanup::cleanup_task(cleanup_db, cleanup_interval).await;
    });

    let ws_addr: SocketAddr = format!("{}:{}", config.server_addr, config.server_port)
        .parse()
        .expect("Invalid server address");

    let listener = TcpListener::bind(ws_addr)
        .await
        .expect("Failed to bind to address");

    tracing::info!(
        "WebSocket server running on ws://{}:{}",
        config.server_addr,
        config.server_port
    );

    let signal_handle = tokio::spawn(wait_for_shutdown(cancel_token.clone()));

    let mut join_set = JoinSet::new();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, peer_addr)) => {
                        let db = Arc::clone(&db);
                        let online_users = Arc::clone(&online_users);
                        let config = Arc::clone(&config);
                        let tokens = tokens.clone();
                        let rate_limiter = rate_limiter.clone();
                        let token = child_token.clone();
                        let model_detector = model_detector.clone();

                        join_set.spawn(async move {
                            match accept_async(stream).await {
                                Ok(ws) => {
                                    handle_connection(
                                        ws,
                                        db,
                                        online_users,
                                        config,
                                        tokens,
                                        rate_limiter,
                                        model_detector,
                                        token,
                                    )
                                    .await
                                }
                                Err(e) => tracing::error!(
                                    "WebSocket handshake error from {}: {}",
                                    peer_addr,
                                    e
                                ),
                            }
                        });
                    }
                    Err(e) => tracing::error!("Failed to accept connection: {}", e),
                }
            }
            _ = child_token.cancelled() => {
                tracing::info!("Shutdown signal received, stopping accept loop");
                break;
            }
        }
    }

    tracing::info!("Waiting for active connections to finish...");

    let shutdown_result = timeout(Duration::from_secs(config.shutdown_timeout_secs), async {
        while let Some(join_result) = join_set.join_next().await {
            if let Err(e) = join_result {
                tracing::error!("Task panicked: {:?}", e);
            }
        }
    })
    .await;

    if shutdown_result.is_err() {
        tracing::warn!("Shutdown timeout reached, forcing exit");
    } else {
        tracing::info!("All connections closed gracefully");
    }

    let mut http_server_handle = Some(http_server);
    tokio::select! {
        result = async { http_server_handle.take().unwrap().await } => {
            match result {
                Ok(()) => tracing::info!("HTTP server shut down gracefully"),
                Err(e) => tracing::error!("HTTP server task failed: {:?}", e),
            }
        }
        _ = tokio::time::sleep(Duration::from_secs(config.shutdown_timeout_secs)) => {
            tracing::warn!("HTTP server shutdown timed out, forcing abort");
            if let Some(handle) = http_server_handle.take() {
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    signal_handle.abort();

    tracing::info!("Server shutdown complete");
}

#[cfg(unix)]
async fn wait_for_shutdown(cancel_token: CancellationToken) {
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, initiating shutdown...");
            cancel_token.cancel();
        }
        _ = sigint.recv() => {
            tracing::info!("Received SIGINT (Ctrl+C), initiating shutdown...");
            cancel_token.cancel();
        }
    }
}

#[cfg(windows)]
async fn wait_for_shutdown(cancel_token: CancellationToken) {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");
    tracing::info!("Received Ctrl+C, initiating shutdown...");
    cancel_token.cancel();
}
