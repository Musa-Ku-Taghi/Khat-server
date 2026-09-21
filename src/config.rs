use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

const DEFAULT_CONFIG_FILE: &str = "config.json";

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub server_addr: String,
    pub server_port: u16,
    pub http_port: u16,
    pub shutdown_timeout_secs: u64,

    pub db_path: String,
    pub db_pool_max_size: u32,
    pub db_journal_mode: String,
    pub db_synchronous: String,

    pub iccid_pepper: String,
    pub allowed_ccs: Vec<String>,
    pub bcrypt_cost: u32,
    pub max_iccid_length: usize,
    pub max_username_length: usize,
    pub min_password_length: usize,
    pub max_password_length: usize,

    pub ping_interval_secs: u64,
    pub pong_timeout_secs: u64,

    pub search_results_limit: usize,
    pub chunk_size: u64,

    pub rate_limit_enabled: bool,
    pub rate_limit_requests_per_minute: u32,
    pub rate_limit_cleanup_interval_secs: u64,

    pub file_storage_dir: String,
    pub temp_upload_dir: String,
    pub file_upload_temp_expiry_secs: u64,
    pub file_max_size_bytes: u64,
    pub allowed_mime_types: Vec<String>,
    pub max_filename_length: usize,
    pub file_rename_sync_delay_ms: u64,
    pub cleanup_interval_secs: u64,

    pub thumbnail_scale_percent: u8,
    pub thumbnail_format: String,
    pub thumbnail_quality: u8,
    pub thumbnail_suffix: String,
    pub video_thumbnail_seconds: u64,
    pub ffmpeg_path: String,
    pub thumb_url_path: String,
    pub pdf_thumbnail_dpi: u32,
    pub pdftoppm_path: String,
    pub font_thumbnail_text: String,
    pub font_thumbnail_bg: String,
    pub font_thumbnail_fg: String,

    pub hash_subdir_depth: usize,
    pub temp_token_bytes: usize,

    pub upload_url_scheme: String,

    pub model_enabled: bool,
    pub model_dir: String,

    pub face_model_enabled: bool,
    pub face_model_dir: String,
    pub face_verify_threshold: f32,

    pub debug: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_addr: "0.0.0.0".to_string(),
            server_port: 8765,
            http_port: 8080,
            shutdown_timeout_secs: 10,

            db_path: "users.db".to_string(),
            db_pool_max_size: 20,
            db_journal_mode: "WAL".to_string(),
            db_synchronous: "NORMAL".to_string(),

            iccid_pepper: "".to_string(),
            allowed_ccs: vec!["98".to_string(), "01".to_string(), "31".to_string()],
            bcrypt_cost: bcrypt::DEFAULT_COST,
            max_iccid_length: 20,
            max_username_length: 50,
            min_password_length: 6,
            max_password_length: 128,

            ping_interval_secs: 30,
            pong_timeout_secs: 15,

            search_results_limit: 5,
            chunk_size: 50,

            rate_limit_enabled: false,
            rate_limit_requests_per_minute: 60,
            rate_limit_cleanup_interval_secs: 120,

            file_storage_dir: "uploads".to_string(),
            temp_upload_dir: "temp_uploads".to_string(),
            file_upload_temp_expiry_secs: 300,
            file_max_size_bytes: 100 * 1024 * 1024,
            allowed_mime_types: default_allowed_mime_types(),
            max_filename_length: 255,
            file_rename_sync_delay_ms: 100,
            cleanup_interval_secs: 3600,

            thumbnail_scale_percent: 50,
            thumbnail_format: "jpeg".to_string(),
            thumbnail_quality: 80,
            thumbnail_suffix: "thumb".to_string(),
            video_thumbnail_seconds: 0,
            ffmpeg_path: "ffmpeg".to_string(),
            thumb_url_path: "/thumb".to_string(),
            pdf_thumbnail_dpi: 50,
            pdftoppm_path: "pdftoppm".to_string(),
            font_thumbnail_text: "Aa".to_string(),
            font_thumbnail_bg: "#FFFFFF".to_string(),
            font_thumbnail_fg: "#000000".to_string(),

            hash_subdir_depth: 2,
            temp_token_bytes: 16,

            upload_url_scheme: "http".to_string(),

            model_enabled: false,
            model_dir: "./spam_hate_detector_xlm_roberta_final".to_string(),

            face_model_enabled: false,
            face_model_dir: "./face_model".to_string(),
            face_verify_threshold: 0.61,

            debug: false,
        }
    }
}

impl Config {
    pub fn load_or_create() -> Self {
        if Path::new(DEFAULT_CONFIG_FILE).exists() {
            let content =
                fs::read_to_string(DEFAULT_CONFIG_FILE).expect("Failed to read config file");
            serde_json::from_str(&content).expect("Failed to parse config JSON")
        } else {
            let default = Self::default();
            let json =
                serde_json::to_string_pretty(&default).expect("Failed to serialize default config");
            fs::write(DEFAULT_CONFIG_FILE, json).expect("Failed to write default config file");
            default
        }
    }

    pub fn validate(&self) {
        require(
            self.shutdown_timeout_secs > 0,
            "shutdown_timeout_secs must be > 0",
        );
        require(self.chunk_size > 0, "chunk_size must be > 0");

        if self.model_enabled {
            require(
                !self.model_dir.is_empty(),
                "model_dir must not be empty when model_enabled is true",
            );
        }

        if self.face_model_enabled {
            require(
                !self.face_model_dir.is_empty(),
                "face_model_dir must not be empty when face_model_enabled is true",
            );
        }
        require(
            self.face_verify_threshold > 0.0 && self.face_verify_threshold <= 2.0,
            "face_verify_threshold must be in (0.0, 2.0]",
        );

        require(self.db_pool_max_size > 0, "db_pool_max_size must be > 0");
        require_one_of(
            &self.db_journal_mode,
            &["WAL", "DELETE", "TRUNCATE", "MEMORY"],
            "db_journal_mode",
        );
        require_one_of(
            &self.db_synchronous,
            &["OFF", "NORMAL", "FULL"],
            "db_synchronous",
        );

        require(
            !self.allowed_ccs.is_empty(),
            "allowed_ccs must not be empty",
        );
        require(
            (4..=31).contains(&self.bcrypt_cost),
            "bcrypt_cost must be between 4 and 31",
        );
        require(self.max_iccid_length > 0, "max_iccid_length must be > 0");
        require(
            self.max_username_length > 0,
            "max_username_length must be > 0",
        );
        require(
            self.min_password_length > 0,
            "min_password_length must be > 0",
        );
        require(
            self.max_password_length > 0,
            "max_password_length must be > 0",
        );
        require(
            self.min_password_length <= self.max_password_length,
            "min_password_length > max_password_length",
        );

        require(
            self.ping_interval_secs > 0,
            "ping_interval_secs must be > 0",
        );
        require(self.pong_timeout_secs > 0, "pong_timeout_secs must be > 0");

        require(
            self.search_results_limit > 0,
            "search_results_limit must be > 0",
        );

        if self.rate_limit_enabled {
            require(
                self.rate_limit_requests_per_minute > 0,
                "rate_limit_requests_per_minute must be > 0 when rate limiting is enabled",
            );
            require(
                self.rate_limit_cleanup_interval_secs > 0,
                "rate_limit_cleanup_interval_secs must be > 0 when rate limiting is enabled",
            );
        }

        require(
            !self.file_storage_dir.is_empty(),
            "file_storage_dir must not be empty",
        );
        require(
            !self.temp_upload_dir.is_empty(),
            "temp_upload_dir must not be empty",
        );
        require(
            self.file_upload_temp_expiry_secs > 0,
            "file_upload_temp_expiry_secs must be > 0",
        );
        require(
            self.file_max_size_bytes > 0,
            "file_max_size_bytes must be > 0",
        );
        require(
            !self.allowed_mime_types.is_empty(),
            "allowed_mime_types must not be empty",
        );
        require(
            self.max_filename_length > 0,
            "max_filename_length must be > 0",
        );
        require(
            self.file_rename_sync_delay_ms <= 10000,
            "file_rename_sync_delay_ms too large (max 10000 ms)",
        );
        require(
            self.cleanup_interval_secs > 0,
            "cleanup_interval_secs must be > 0",
        );

        require(
            (1..=100).contains(&self.thumbnail_scale_percent),
            "thumbnail_scale_percent must be between 1 and 100",
        );
        require_one_of(
            &self.thumbnail_format,
            &["jpeg", "png", "webp"],
            "thumbnail_format",
        );
        require(
            (1..=100).contains(&self.thumbnail_quality),
            "thumbnail_quality must be between 1 and 100",
        );
        require(
            !self.thumbnail_suffix.is_empty(),
            "thumbnail_suffix must not be empty",
        );
        require(
            !self.ffmpeg_path.is_empty(),
            "ffmpeg_path must not be empty",
        );
        require(
            !self.thumb_url_path.is_empty(),
            "thumb_url_path must not be empty",
        );

        require(
            (1..=300).contains(&self.pdf_thumbnail_dpi),
            "pdf_thumbnail_dpi must be between 1 and 300",
        );
        require(
            !self.pdftoppm_path.is_empty(),
            "pdftoppm_path must not be empty",
        );
        require(
            !self.font_thumbnail_text.is_empty(),
            "font_thumbnail_text must not be empty",
        );
        require(
            !self.font_thumbnail_bg.is_empty(),
            "font_thumbnail_bg must not be empty",
        );
        require(
            !self.font_thumbnail_fg.is_empty(),
            "font_thumbnail_fg must not be empty",
        );

        require(self.hash_subdir_depth > 0, "hash_subdir_depth must be > 0");
        require(self.temp_token_bytes > 0, "temp_token_bytes must be > 0");
        require_one_of(
            &self.upload_url_scheme,
            &["http", "https"],
            "upload_url_scheme",
        );
    }
}

#[inline]
fn require(cond: bool, msg: &str) {
    if !cond {
        panic!("Config: {msg}");
    }
}

#[inline]
fn require_one_of(value: &str, allowed: &[&str], field: &str) {
    if !allowed.contains(&value) {
        panic!("Config: {field} must be one of: {}", allowed.join(", "));
    }
}

fn default_allowed_mime_types() -> Vec<String> {
    [
        "image/jpeg",
        "image/png",
        "image/gif",
        "image/webp",
        "image/svg+xml",
        "image/bmp",
        "image/tiff",
        "image/heic",
        "image/avif",
        "video/mp4",
        "video/webm",
        "video/quicktime",
        "video/x-msvideo",
        "video/x-matroska",
        "video/mpeg",
        "video/3gpp",
        "audio/mpeg",
        "audio/mp4",
        "audio/x-m4a",
        "audio/aac",
        "audio/flac",
        "audio/ogg",
        "audio/opus",
        "audio/wav",
        "audio/x-wav",
        "audio/aiff",
        "audio/x-aiff",
        "font/ttf",
        "font/otf",
        "font/woff",
        "font/woff2",
        "application/font-woff",
        "application/x-font-ttf",
        "application/x-font-otf",
        "application/pdf",
        "application/msword",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "application/vnd.ms-excel",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "application/vnd.ms-powerpoint",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "application/vnd.oasis.opendocument.text",
        "application/vnd.oasis.opendocument.spreadsheet",
        "application/vnd.oasis.opendocument.presentation",
        "application/rtf",
        "application/epub+zip",
        "application/zip",
        "application/x-rar-compressed",
        "application/x-7z-compressed",
        "application/x-tar",
        "application/gzip",
        "application/x-bzip2",
        "text/plain",
        "text/csv",
        "text/json",
        "application/json",
        "text/xml",
        "application/xml",
        "text/yaml",
        "text/x-yaml",
        "text/toml",
        "text/markdown",
        "application/octet-stream",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
