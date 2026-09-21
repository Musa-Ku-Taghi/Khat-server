pub mod content_settings;
pub mod conversation_lock;
pub mod errors;
pub mod file;
pub mod message;
pub mod pin;
pub mod profile;
pub mod service;
pub mod user;

pub use errors::DbError;
pub use service::DbService;

use std::sync::Arc;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use sha2::{Digest, Sha256};

use crate::config::Config;

pub struct Database {
    pool: Pool<SqliteConnectionManager>,
    config: Arc<Config>,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub id: i64,
    pub hash: String,
    pub original_name: String,
    pub mime_type: String,
    pub size: i64,
    pub storage_path: String,
    pub thumb_path: Option<String>,
    pub created_at: String,
    pub is_temp: bool,
    pub temp_expires_at: Option<String>,
    pub referenced_count: i64,
    pub requester_ip: Option<String>,
    pub user_id: Option<i64>,
    pub upload_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: i64,
    pub hash: String,
    pub storage_path: String,
    pub thumb_path: Option<String>,
    pub mime_type: String,
    pub original_name: String,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct TempFileEntry {
    pub id: i64,
    pub storage_path: String,
    pub thumb_path: Option<String>,
    pub temp_expires_at: Option<String>,
}

impl Database {
    pub fn new(path: &str, config: Arc<Config>) -> Result<Self, DbError> {
        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .max_size(config.db_pool_max_size)
            .build(manager)?;

        let conn = pool.get()?;
        let pragma_sql = format!(
            "PRAGMA journal_mode = {};
             PRAGMA synchronous = {};",
            config.db_journal_mode, config.db_synchronous
        );
        conn.execute_batch(&pragma_sql)?;

        Self::init_schema(&conn)?;

        Ok(Database { pool, config })
    }

    fn init_schema(conn: &PooledConnection<SqliteConnectionManager>) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                iccid TEXT NOT NULL UNIQUE,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_username ON users(username);
            CREATE INDEX IF NOT EXISTS idx_iccid ON users(iccid);

            CREATE TABLE IF NOT EXISTS direct_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                sender_id INTEGER NOT NULL,
                recipient_id INTEGER NOT NULL,
                content TEXT NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
                read BOOLEAN DEFAULT FALSE,
                edited_at TIMESTAMP,
                FOREIGN KEY (sender_id) REFERENCES users(id),
                FOREIGN KEY (recipient_id) REFERENCES users(id)
            );
            CREATE INDEX IF NOT EXISTS idx_dm_sender ON direct_messages(sender_id);
            CREATE INDEX IF NOT EXISTS idx_dm_recipient ON direct_messages(recipient_id);
            CREATE INDEX IF NOT EXISTS idx_dm_timestamp ON direct_messages(timestamp);

            CREATE TABLE IF NOT EXISTS file_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hash TEXT NOT NULL UNIQUE,
                original_name TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                storage_path TEXT NOT NULL,
                thumb_path TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                is_temp BOOLEAN NOT NULL DEFAULT 0,
                temp_expires_at TIMESTAMP,
                referenced_count INTEGER NOT NULL DEFAULT 0,
                user_id INTEGER REFERENCES users(id),
                upload_type TEXT DEFAULT 'message',
                upload_token TEXT,
                requester_ip TEXT
            );

            CREATE TABLE IF NOT EXISTS file_references (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL,
                message_id INTEGER NOT NULL,
                FOREIGN KEY (file_id) REFERENCES file_metadata(id) ON DELETE CASCADE,
                FOREIGN KEY (message_id) REFERENCES direct_messages(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS pinned_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user1_id INTEGER NOT NULL,
                user2_id INTEGER NOT NULL,
                message_id INTEGER NOT NULL,
                pinned_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (message_id) REFERENCES direct_messages(id) ON DELETE CASCADE,
                UNIQUE(user1_id, user2_id, message_id)
            );
            CREATE INDEX IF NOT EXISTS idx_pinned_users ON pinned_messages(user1_id, user2_id);

            CREATE INDEX IF NOT EXISTS idx_file_refs_file_id ON file_references(file_id);
            CREATE INDEX IF NOT EXISTS idx_file_hash ON file_metadata(hash);
            CREATE INDEX IF NOT EXISTS idx_file_temp ON file_metadata(is_temp, temp_expires_at);
            CREATE INDEX IF NOT EXISTS idx_file_upload_token ON file_metadata(upload_token);

            CREATE TABLE IF NOT EXISTS profile_pictures (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                file_id INTEGER NOT NULL,
                is_primary BOOLEAN NOT NULL DEFAULT 0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (file_id) REFERENCES file_metadata(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_profile_pics_user ON profile_pictures(user_id);
            
            CREATE TABLE IF NOT EXISTS content_analysis_settings (
                user_id INTEGER PRIMARY KEY,
                spam BOOLEAN NOT NULL DEFAULT 0,
                obscene BOOLEAN NOT NULL DEFAULT 0,
                hate BOOLEAN NOT NULL DEFAULT 0,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            );
            
            CREATE TABLE IF NOT EXISTS conversation_locks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_id INTEGER NOT NULL,
                partner_id INTEGER NOT NULL,
                hash_0 TEXT NOT NULL,
                hash_1 TEXT NOT NULL,
                hash_2 TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (partner_id) REFERENCES users(id) ON DELETE CASCADE,
                UNIQUE(owner_id, partner_id)
            );

            CREATE TABLE IF NOT EXISTS conversation_lock_refs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                lock_id INTEGER NOT NULL,
                slot INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                FOREIGN KEY (lock_id) REFERENCES conversation_locks(id) ON DELETE CASCADE,
                UNIQUE(lock_id, slot)
            );

            CREATE TABLE IF NOT EXISTS face_lock_pending_uploads (
                token TEXT PRIMARY KEY,
                owner_id INTEGER NOT NULL,
                partner_id INTEGER NOT NULL,
                slot INTEGER NOT NULL,
                FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (partner_id) REFERENCES users(id) ON DELETE CASCADE
            );",
        )?;

        Ok(())
    }

    pub fn get_conn(&self) -> Result<PooledConnection<SqliteConnectionManager>, DbError> {
        Ok(self.pool.get()?)
    }

    fn validate_password(&self, password: &str) -> Result<(), DbError> {
        validate_input(password, self.config.max_password_length, "password")?;
        if password.len() < self.config.min_password_length {
            return Err(DbError::InvalidInput(format!(
                "password must be at least {} characters",
                self.config.min_password_length
            )));
        }
        Ok(())
    }

    fn hash_iccid(&self, iccid: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(iccid.as_bytes());
        hasher.update(self.config.iccid_pepper.as_bytes());
        hex::encode(hasher.finalize())
    }
}

pub(crate) fn validate_input(
    input: &str,
    max_length: usize,
    field_name: &str,
) -> Result<(), DbError> {
    if input.is_empty() || input.len() > max_length {
        return Err(DbError::InvalidInput(format!(
            "{field_name} must be between 1 and {max_length} characters"
        )));
    }
    Ok(())
}
