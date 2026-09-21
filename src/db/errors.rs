use std::fmt;

use bcrypt::BcryptError;
use r2d2::Error as R2D2Error;
use rusqlite::Error as RusqliteError;
use serde_json::Error as SerdeJsonError;
use tokio::task::JoinError;

#[derive(Debug)]
pub enum DbError {
    IccidExists,
    UsernameExists,
    UsernameNotFound,
    PasswordIncorrect,
    InvalidInput(String),
    DatabaseError(String),
    FileNotFound,
    FileHashExists,
    FileTempExpired,
}

impl From<RusqliteError> for DbError {
    fn from(err: RusqliteError) -> Self {
        DbError::DatabaseError(err.to_string())
    }
}

impl From<R2D2Error> for DbError {
    fn from(err: R2D2Error) -> Self {
        DbError::DatabaseError(err.to_string())
    }
}

impl From<BcryptError> for DbError {
    fn from(err: BcryptError) -> Self {
        DbError::DatabaseError(err.to_string())
    }
}

impl From<SerdeJsonError> for DbError {
    fn from(err: SerdeJsonError) -> Self {
        DbError::DatabaseError(err.to_string())
    }
}

impl From<JoinError> for DbError {
    fn from(err: JoinError) -> Self {
        DbError::DatabaseError(err.to_string())
    }
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::IccidExists => f.write_str("ICCID already exists"),
            DbError::UsernameExists => f.write_str("Username already exists"),
            DbError::UsernameNotFound => f.write_str("Username not found"),
            DbError::PasswordIncorrect => f.write_str("Password incorrect"),
            DbError::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
            DbError::DatabaseError(e) => write!(f, "Database error: {e}"),
            DbError::FileNotFound => f.write_str("File not found"),
            DbError::FileHashExists => f.write_str("File with this hash already exists"),
            DbError::FileTempExpired => f.write_str("Temporary upload token has expired"),
        }
    }
}

impl std::error::Error for DbError {}
