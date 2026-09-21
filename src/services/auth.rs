use std::sync::Arc;

use tracing::{error, info, warn};

use crate::auth::TokenMap;
use crate::config::Config;
use crate::db::{Database, DbError, DbService};
use crate::models::{LoginResponse, ResponseStatus, SignupResponse};
use crate::validators::validate_iccid;

pub async fn signup(
    db: Arc<Database>,
    tokens: TokenMap,
    iccid: String,
    username: String,
    password: String,
    config: &Config,
) -> SignupResponse {
    if let Err(err) = validate_iccid(&iccid, &config.allowed_ccs) {
        warn!("ICCID validation failed: {err}");
        return SignupResponse {
            msg_type: "signup_response".to_string(),
            status: ResponseStatus::IccidError,
            token: None,
        };
    }

    let result = db
        .call({
            let iccid = iccid.clone();
            let username = username.clone();
            let password = password.clone();
            move |d| d.signup(&iccid, &username, &password)
        })
        .await;

    let (status, token) = match result {
        Ok(()) => {
            info!("Signup successful: {username} ({iccid})");
            let token = crate::auth::store_token(&tokens, &username).await;
            (ResponseStatus::Success, Some(token))
        }
        Err(DbError::IccidExists) => {
            warn!("Signup failed: ICCID already exists");
            (ResponseStatus::IccidError, None)
        }
        Err(DbError::UsernameExists) => {
            warn!("Signup failed: Username already exists");
            (ResponseStatus::UsernameError, None)
        }
        Err(DbError::InvalidInput(msg)) => {
            warn!("Signup failed: Invalid input - {msg}");
            (ResponseStatus::InvalidInput, None)
        }
        Err(DbError::DatabaseError(e)) => {
            error!("Database error during signup: {e}");
            (ResponseStatus::Error, None)
        }
        Err(_) => {
            error!("Unexpected error during signup");
            (ResponseStatus::Error, None)
        }
    };

    SignupResponse {
        msg_type: "signup_response".to_string(),
        status,
        token,
    }
}

pub async fn login(
    db: Arc<Database>,
    tokens: TokenMap,
    username: String,
    password: String,
) -> LoginResponse {
    let result = db
        .call({
            let username = username.clone();
            let password = password.clone();
            move |d| d.login(&username, &password)
        })
        .await;

    let (status, token) = match result {
        Ok(()) => {
            info!("Login successful: {username}");
            let token = crate::auth::store_token(&tokens, &username).await;
            (ResponseStatus::Success, Some(token))
        }
        Err(DbError::UsernameNotFound) => {
            warn!("Login failed: Username not found ({username})");
            (ResponseStatus::UsernameError, None)
        }
        Err(DbError::PasswordIncorrect) => {
            warn!("Login failed: Password incorrect for {username}");
            (ResponseStatus::PasswordError, None)
        }
        Err(DbError::InvalidInput(msg)) => {
            warn!("Login failed: Invalid input - {msg}");
            (ResponseStatus::InvalidInput, None)
        }
        Err(e) => {
            error!("Database error during login: {e}");
            (ResponseStatus::Error, None)
        }
    };

    LoginResponse {
        msg_type: "signin_response".to_string(),
        status,
        token,
    }
}
