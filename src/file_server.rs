use std::net::SocketAddr;
use std::path::{Path as StdPath, PathBuf};
use std::str::from_utf8;
use std::sync::Arc;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use apk_info::Apk;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{body::Body, Router};
use chrono::{Local, NaiveDateTime};
use futures_util::{StreamExt, TryStreamExt};
use image::imageops::FilterType;
use infer::Infer;
use lofty::file::{FileType, TaggedFileExt};
use lofty::probe::Probe;
use resvg::{tiny_skia, usvg};
use tokio::fs::{create_dir_all, remove_file, File};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use tracing::{error, info, warn};

use crate::auth::{validate_token, TokenMap};
use crate::config::Config;
use crate::db::{conversation_lock::SLOT_PROBE, Database, DbService, FileMetadata};
use crate::error::{AppError, AppResult};
use crate::face_detector::{classify, distance_to_prototype, FaceDetector, Verdict};
use crate::models::FaceDetectionResponse;
use crate::online::OnlineUsers;
use crate::utils;

pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub tokens: TokenMap,
    pub online_users: Arc<tokio::sync::RwLock<OnlineUsers>>,
    pub face_detector: Arc<tokio::sync::Mutex<FaceDetector>>,
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let upload_limit = state.config.file_max_size_bytes as usize;
    Router::new()
        .route(
            "/upload/:token",
            post(upload_handler).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route("/files/:id", get(download_file))
        .route("/thumb/:id", get(download_thumb))
        .route("/profile_pics/:id", get(download_profile_pic))
        .with_state(state)
}

const PDF_MIME: &str = "application/pdf";

fn is_font_mime(mime: &str) -> bool {
    matches!(
        mime,
        "font/ttf"
            | "font/otf"
            | "font/woff"
            | "font/woff2"
            | "application/font-woff"
            | "application/x-font-ttf"
            | "application/x-font-otf"
    )
}

fn parse_hex_color(s: &str) -> Option<image::Rgb<u8>> {
    let s = s.strip_prefix('#').unwrap_or(s);
    let (r, g, b) = match s.len() {
        3 => {
            let r = u8::from_str_radix(&s[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&s[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&s[2..3].repeat(2), 16).ok()?;
            (r, g, b)
        }
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            (r, g, b)
        }
        _ => return None,
    };
    Some(image::Rgb([r, g, b]))
}

fn extension_for_format(format_str: &str) -> &'static str {
    match format_str {
        "png" => "png",
        "webp" => "webp",
        _ => "jpg",
    }
}

fn save_thumb(
    img: &image::DynamicImage,
    path: &StdPath,
    format_str: &str,
    quality: u8,
    suffix: &str,
) -> Option<PathBuf> {
    let ext = extension_for_format(format_str);
    let thumb_path = path.with_extension(format!("{suffix}.{ext}"));
    let file = std::fs::File::create(&thumb_path).ok()?;

    let result = match format_str {
        "png" => {
            let encoder = image::codecs::png::PngEncoder::new_with_quality(
                file,
                image::codecs::png::CompressionType::Default,
                image::codecs::png::FilterType::Adaptive,
            );
            img.write_with_encoder(encoder)
        }
        "webp" => {
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(file);
            img.write_with_encoder(encoder)
        }
        _ => {
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
            img.write_with_encoder(encoder)
        }
    };

    result.ok().map(|_| thumb_path)
}

fn read_cover_art(
    path: &StdPath,
    file_type: FileType,
    format_str: &str,
    quality: u8,
    suffix: &str,
    scale: f64,
) -> Option<PathBuf> {
    let probe = Probe::open(path).ok()?.set_file_type(file_type);

    let tagged = match probe.read() {
        Ok(t) => t,
        Err(e) => {
            info!(
                "No readable audio tags in {} (as {:?}): {}",
                path.display(),
                file_type,
                e
            );
            return None;
        }
    };

    let tag = match tagged.primary_tag() {
        Some(t) => t,
        None => {
            info!("No primary tag in {}", path.display());
            return None;
        }
    };

    let picture = match tag.pictures().first() {
        Some(p) => p,
        None => {
            info!("No cover art in {}", path.display());
            return None;
        }
    };

    let data = picture.data();
    if data.is_empty() {
        return None;
    }

    let img = match image::load_from_memory(data) {
        Ok(i) => i,
        Err(e) => {
            warn!("Failed to decode embedded cover art: {e}");
            return None;
        }
    };

    let new_w = ((img.width() as f64 * scale) as u32).max(1);
    let new_h = ((img.height() as f64 * scale) as u32).max(1);
    let thumb = img.resize(new_w, new_h, FilterType::Lanczos3);

    save_thumb(&thumb, path, format_str, quality, suffix)
}

async fn upload_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> AppResult<Response> {
    let pending_face = state
        .db
        .call({
            let t = token.clone();
            move |d| {
                let pending = d.get_pending_face_upload(&t)?;
                let meta = d.get_temp_upload_by_token(&t)?;
                Ok::<_, crate::db::DbError>((pending, meta))
            }
        })
        .await
        .map_err(|_| AppError::Internal)?;

    if let (Some(pending), Some(meta)) = pending_face {
        validate_upload_request(&state, &meta, &addr)?;

        let response =
            handle_face_lock_upload(Arc::clone(&state), token.clone(), pending, multipart).await;

        if response.is_ok() {
            let _ = state
                .db
                .call({
                    let t = token.clone();
                    move |d| d.delete_pending_face_upload(&t)
                })
                .await;

            let id = meta.id;
            let _ = state.db.call(move |d| d.delete_file_metadata(id)).await;
        }

        return response;
    }

    let meta = load_pending_upload(&state, &token).await?;
    validate_upload_request(&state, &meta, &addr)?;

    let field = multipart
        .next_field()
        .await
        .map_err(|_| AppError::BadRequest("Failed to read field".into()))?
        .ok_or_else(|| AppError::BadRequest("No file provided".into()))?;

    let client_mime = field.content_type().map(str::to_string);

    let temp_path = stream_to_temp(&state, &token, field).await?;

    let detected_mime = detect_mime(&temp_path)
        .await?
        .ok_or_else(|| AppError::BadRequest("Unable to detect file MIME type".into()))?;

    if !state.config.allowed_mime_types.contains(&detected_mime) {
        warn!(
            "Detected MIME not allowed: {} (client sent: {:?})",
            detected_mime, client_mime
        );
        let _ = remove_file(&temp_path).await;
        return Err(AppError::UnsupportedMediaType);
    }

    let final_storage_path = compute_storage_path(&state, &meta.hash).await?;

    ensure_path_safe(&state, &temp_path).await?;

    let temp_thumb = match generate_thumbnail(
        &temp_path,
        &detected_mime,
        &meta.original_name,
        &state.config,
    )
    .await
    {
        Ok(thumb) => thumb,
        Err(e) => {
            warn!("Thumbnail generation failed for {}: {:?}", detected_mime, e);
            None
        }
    };

    tokio::fs::rename(&temp_path, &final_storage_path)
        .await
        .map_err(|_| AppError::Internal)?;

    tokio::time::sleep(std::time::Duration::from_millis(
        state.config.file_rename_sync_delay_ms,
    ))
    .await;

    if let Ok(md) = tokio::fs::metadata(&final_storage_path).await {
        info!("Final file size: {} bytes", md.len());
    }

    let final_thumb_path = move_thumbnail(&state, &final_storage_path, temp_thumb).await?;

    finalize_upload(
        &state,
        meta.id,
        &final_storage_path,
        final_thumb_path.as_deref(),
        &detected_mime,
    )
    .await?;

    if let Some(user_id) = meta.user_id {
        if meta.upload_type.as_deref() == Some("profile") {
            let id = meta.id;
            state
                .db
                .call(move |d| d.add_profile_picture(user_id, id))
                .await
                .map_err(|_| AppError::Internal)?;
        }
    }

    let default_host = format!("{}:{}", state.config.server_addr, state.config.http_port);
    let host = headers
        .get("Host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(&default_host);
    let scheme = &state.config.upload_url_scheme;

    let upload_url = format!("{scheme}://{host}/upload/{token}");
    let thumb_url = final_thumb_path
        .is_some()
        .then(|| format!("{}/{}", state.config.thumb_url_path, meta.id));

    Ok(axum::Json(serde_json::json!({
        "type": "file_upload_response",
        "status": "success",
        "upload_url": upload_url,
        "file_id": meta.id,
        "thumb_url": thumb_url,
    }))
    .into_response())
}

async fn load_pending_upload(state: &AppState, token: &str) -> AppResult<FileMetadata> {
    let token_owned = token.to_string();
    state
        .db
        .call(move |d| d.get_temp_upload_by_token(&token_owned))
        .await
        .map_err(|_| AppError::NotFound)?
        .ok_or(AppError::NotFound)
}

fn validate_upload_request(
    state: &AppState,
    meta: &FileMetadata,
    addr: &SocketAddr,
) -> AppResult<()> {
    if meta.original_name.len() > state.config.max_filename_length {
        return Err(AppError::BadRequest("Filename too long".into()));
    }

    let client_ip = addr.ip().to_string();
    match meta.requester_ip.as_deref() {
        Some(stored) if stored == client_ip => {}
        _ => return Err(AppError::Unauthorized),
    }

    let expires_str = meta.temp_expires_at.as_deref().ok_or(AppError::Internal)?;
    let expires = NaiveDateTime::parse_from_str(expires_str, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| AppError::Internal)?;
    if Local::now().naive_local() > expires {
        return Err(AppError::BadRequest("Upload token expired".into()));
    }

    Ok(())
}

async fn stream_to_temp(
    state: &AppState,
    token: &str,
    field: axum::extract::multipart::Field<'_>,
) -> AppResult<PathBuf> {
    let temp_dir = PathBuf::from(&state.config.temp_upload_dir);
    create_dir_all(&temp_dir)
        .await
        .map_err(|_| AppError::Internal)?;

    let temp_path = temp_dir.join(token);
    let mut writer = File::create(&temp_path)
        .await
        .map_err(|_| AppError::Internal)?;

    let mut written = 0u64;
    let mut stream = field.into_stream();

    while let Some(chunk) = stream.next().await {
        let data = chunk.map_err(|_| AppError::BadRequest("Failed to read chunk".into()))?;
        let len = data.len() as u64;
        if written + len > state.config.file_max_size_bytes {
            let _ = remove_file(&temp_path).await;
            return Err(AppError::PayloadTooLarge);
        }
        writer
            .write_all(&data)
            .await
            .map_err(|_| AppError::Internal)?;
        written += len;
    }

    writer.flush().await.map_err(|_| AppError::Internal)?;
    Ok(temp_path)
}

async fn detect_mime(path: &StdPath) -> AppResult<Option<String>> {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let infer = Infer::new();
        let data = std::fs::read(&path).ok()?;
        if let Some(info) = infer.get(&data) {
            return Some(info.mime_type().to_string());
        }
        if from_utf8(&data).is_ok() {
            return Some("text/plain".to_string());
        }
        Some("application/octet-stream".to_string())
    })
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(result)
}

async fn compute_storage_path(state: &AppState, hash: &str) -> AppResult<PathBuf> {
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest("Invalid hash format".into()));
    }
    let depth = state.config.hash_subdir_depth;
    if hash.len() < depth {
        return Err(AppError::BadRequest("Hash too short".into()));
    }
    let (sub, rest) = hash.split_at(depth);

    let dir = PathBuf::from(&state.config.file_storage_dir).join(sub);
    create_dir_all(&dir).await.map_err(|_| AppError::Internal)?;
    Ok(dir.join(rest))
}

async fn ensure_path_safe(state: &AppState, temp_path: &StdPath) -> AppResult<()> {
    let base = PathBuf::from(&state.config.temp_upload_dir);
    if !utils::is_path_safe(&base, temp_path) {
        let _ = remove_file(temp_path).await;
        return Err(AppError::BadRequest("Invalid file path".into()));
    }
    Ok(())
}

async fn move_thumbnail(
    state: &AppState,
    final_storage: &StdPath,
    temp_thumb: Option<PathBuf>,
) -> AppResult<Option<PathBuf>> {
    let Some(thumb) = temp_thumb else {
        return Ok(None);
    };
    let ext = thumb.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
    let final_thumb =
        final_storage.with_extension(format!("{}.{ext}", state.config.thumbnail_suffix));
    tokio::fs::rename(&thumb, &final_thumb)
        .await
        .map_err(|_| AppError::Internal)?;
    Ok(Some(final_thumb))
}

async fn finalize_upload(
    state: &AppState,
    id: i64,
    final_storage: &StdPath,
    final_thumb: Option<&StdPath>,
    mime: &str,
) -> AppResult<()> {
    let storage_str = final_storage
        .to_str()
        .ok_or(AppError::Internal)?
        .to_string();
    let thumb_str = final_thumb.map(|p| p.to_str().unwrap_or("").to_string());
    let mime_owned = mime.to_string();

    let result = state
        .db
        .call(move |d| d.finalize_upload(id, &storage_str, thumb_str.as_deref(), &mime_owned))
        .await;

    if result.is_err() {
        let _ = remove_file(final_storage).await;
        if let Some(thumb) = final_thumb {
            let _ = remove_file(thumb).await;
        }
        return Err(AppError::Internal);
    }

    Ok(())
}

async fn download_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let _user = authenticate(&state.tokens, &headers).await?;
    let meta = fetch_file_metadata(&state.db, id).await?;
    if meta.is_temp {
        return Err(AppError::NotFound);
    }
    stream_file(
        &meta.storage_path,
        &meta.mime_type,
        Some(&meta.original_name),
    )
    .await
}

async fn download_thumb(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let _user = authenticate(&state.tokens, &headers).await?;
    let meta = fetch_file_metadata(&state.db, id).await?;

    if meta.is_temp {
        return Err(AppError::NotFound);
    }

    let Some(thumb) = meta.thumb_path else {
        return Err(AppError::NotFound);
    };

    if !StdPath::new(&thumb).exists() {
        return Err(AppError::NotFound);
    }

    let mime = match StdPath::new(&thumb).extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };

    stream_file(&thumb, mime, None).await
}

async fn download_profile_pic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let _user = authenticate(&state.tokens, &headers).await?;
    let meta = fetch_file_metadata(&state.db, id).await?;
    if meta.is_temp {
        return Err(AppError::NotFound);
    }

    let is_profile = state
        .db
        .call(move |d| d.is_profile_picture(id))
        .await
        .map_err(|_| AppError::Internal)?;
    if !is_profile {
        return Err(AppError::NotFound);
    }

    stream_file(&meta.storage_path, &meta.mime_type, None).await
}

async fn stream_file(path: &str, mime: &str, filename: Option<&str>) -> AppResult<Response> {
    let p = StdPath::new(path);
    if !p.exists() {
        return Err(AppError::NotFound);
    }

    let file = File::open(p).await.map_err(|_| AppError::Internal)?;
    let body = Body::from_stream(ReaderStream::new(file));

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime);

    if let Some(name) = filename {
        builder = builder.header(
            "Content-Disposition",
            format!("inline; filename=\"{name}\""),
        );
    }

    builder.body(body).map_err(|_| AppError::Internal)
}

async fn authenticate(tokens: &TokenMap, headers: &HeaderMap) -> AppResult<String> {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;

    validate_token(tokens, token)
        .await
        .ok_or(AppError::Unauthorized)
}

async fn fetch_file_metadata(db: &Arc<Database>, id: i64) -> AppResult<FileMetadata> {
    db.call(move |d| d.get_file_by_id(id))
        .await
        .map_err(|_| AppError::Internal)?
        .ok_or(AppError::NotFound)
}

async fn generate_thumbnail(
    file_path: &StdPath,
    mime_type: &str,
    original_name: &str,
    config: &Config,
) -> AppResult<Option<PathBuf>> {
    let ctx = ThumbCtx {
        scale: config.thumbnail_scale_percent as f64 / 100.0,
        format: config.thumbnail_format.clone(),
        quality: config.thumbnail_quality,
        suffix: config.thumbnail_suffix.clone(),
    };
    if original_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.eq_ignore_ascii_case("apk"))
        .unwrap_or(false)
    {
        return apk_thumb(file_path, ctx).await;
    }

    if mime_type == "image/svg+xml" {
        return svg_thumb(file_path, ctx).await;
    }
    if mime_type.starts_with("image/") {
        return image_thumb(file_path, ctx).await;
    }
    if mime_type.starts_with("video/") {
        return video_thumb(file_path, mime_type, config, ctx).await;
    }
    if mime_type == PDF_MIME {
        return pdf_thumb(file_path, config, ctx).await;
    }
    if mime_type.starts_with("audio/") {
        return audio_thumb(file_path, mime_type, ctx).await;
    }
    if is_font_mime(mime_type) {
        return font_thumb(file_path, config, ctx).await;
    }
    Ok(None)
}

#[derive(Clone)]
struct ThumbCtx {
    scale: f64,
    format: String,
    quality: u8,
    suffix: String,
}

async fn apk_thumb(path: &StdPath, ctx: ThumbCtx) -> AppResult<Option<PathBuf>> {
    let path = path.to_path_buf();

    let result = tokio::task::spawn_blocking(move || {
        let apk = match Apk::new(&path) {
            Ok(apk) => apk,
            Err(e) => {
                warn!("Failed to parse APK {}: {}", path.display(), e);
                return None;
            }
        };

        let mut candidates: Vec<String> = Vec::new();

        if let Some(icon) = apk.get_application_icon() {
            info!("APK manifest icon: {}", icon);
            candidates.push(icon);
        }

        for entry in apk.namelist() {
            let lower = entry.to_ascii_lowercase();

            let image_file = lower.ends_with(".png")
                || lower.ends_with(".webp")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg");

            if !image_file {
                continue;
            }

            let icon_file = lower.contains("icon")
                || lower.contains("launcher")
                || lower.contains("/mipmap")
                || lower.contains("/drawable");

            if icon_file {
                candidates.push(entry.to_string());
            }
        }

        candidates.sort();
        candidates.dedup();

        info!(
            "Searching {} APK icon candidates in {}",
            candidates.len(),
            path.display()
        );

        for icon_path in candidates {
            let (data, _) = match apk.read(&icon_path) {
                Ok(data) => data,
                Err(e) => {
                    warn!(
                        "Failed to read APK candidate {} from {}: {}",
                        icon_path,
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            let img = match image::load_from_memory(&data) {
                Ok(img) => img,
                Err(e) => {
                    warn!(
                        "APK candidate {} is not a decodable image: {}",
                        icon_path, e
                    );
                    continue;
                }
            };

            info!(
                "Found usable APK icon: {} ({:?})",
                icon_path,
                (img.width(), img.height())
            );

            let new_w = ((img.width() as f64 * ctx.scale) as u32).max(1);
            let new_h = ((img.height() as f64 * ctx.scale) as u32).max(1);
            let thumb = img.resize(new_w, new_h, FilterType::Lanczos3);

            return save_thumb(&thumb, &path, &ctx.format, ctx.quality, &ctx.suffix);
        }

        warn!(
            "No usable icon image found anywhere in APK {}",
            path.display()
        );
        None
    })
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(result)
}

async fn svg_thumb(path: &StdPath, ctx: ThumbCtx) -> AppResult<Option<PathBuf>> {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let data = std::fs::read(&path).ok()?;
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;

        let size = tree.size();
        let w = ((size.width() as f64 * ctx.scale) as u32).max(1);
        let h = ((size.height() as f64 * ctx.scale) as u32).max(1);

        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
        let transform = tiny_skia::Transform::from_scale(ctx.scale as f32, ctx.scale as f32);
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        let rgba = image::RgbaImage::from_raw(w, h, pixmap.data().to_vec())?;
        let img = image::DynamicImage::ImageRgba8(rgba);

        save_thumb(&img, &path, &ctx.format, ctx.quality, &ctx.suffix)
    })
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(result)
}

async fn image_thumb(path: &StdPath, ctx: ThumbCtx) -> AppResult<Option<PathBuf>> {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let data = std::fs::read(&path).ok()?;
        let img = image::load_from_memory(&data)
            .map_err(|e| error!("Failed to decode image: {e}"))
            .ok()?;
        let new_w = ((img.width() as f64 * ctx.scale) as u32).max(1);
        let new_h = ((img.height() as f64 * ctx.scale) as u32).max(1);
        let thumb = img.resize(new_w, new_h, FilterType::Lanczos3);
        save_thumb(&thumb, &path, &ctx.format, ctx.quality, &ctx.suffix)
    })
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(result)
}

async fn video_thumb(
    path: &StdPath,
    mime_type: &str,
    config: &Config,
    ctx: ThumbCtx,
) -> AppResult<Option<PathBuf>> {
    const AUDIO_EXTS: &[&str] = &[
        "m4a", "m4b", "mp3", "aac", "ogg", "oga", "opus", "wav", "flac", "wma", "aiff", "aif",
        "alac", "ape",
    ];
    const VIDEO_EXTS: &[&str] = &[
        "mp4", "m4v", "webm", "mov", "mkv", "avi", "3gp", "3g2", "flv", "wmv", "mpg", "mpeg", "ts",
        "m2ts", "ogv",
    ];

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if AUDIO_EXTS.contains(&ext.as_str()) {
        info!(
            "Skipping thumbnail for audio extension '{ext}': {}",
            path.display()
        );
        return Ok(None);
    }

    if !VIDEO_EXTS.contains(&ext.as_str()) {
        let has_video = probe_video_stream(path, config).await;
        if !has_video {
            info!(
                "Skipping thumbnail: no video stream detected in {}",
                path.display()
            );
            if mime_type == "video/mp4" {
                let path = path.to_path_buf();
                let ctx = ctx.clone();
                return tokio::task::spawn_blocking(move || {
                    read_cover_art(
                        &path,
                        FileType::Mp4,
                        &ctx.format,
                        ctx.quality,
                        &ctx.suffix,
                        ctx.scale,
                    )
                })
                .await
                .map_err(|_| AppError::Internal);
            }
            return Ok(None);
        }
    }

    let ext = extension_for_format(&ctx.format);
    let thumb_path = path.with_extension(format!("{}.{ext}", ctx.suffix));
    let ffmpeg = config.ffmpeg_path.clone();
    let seek = config.video_thumbnail_seconds;
    let quality = ctx.quality;
    let scale = ctx.scale;
    let path_str = path.to_string_lossy().into_owned();
    let thumb_str = thumb_path.to_string_lossy().into_owned();

    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&ffmpeg)
            .args([
                "-ss",
                &seek.to_string(),
                "-i",
                &path_str,
                "-vframes",
                "1",
                "-vf",
                &format!("scale=iw*{scale}:ih*{scale}"),
                "-q:v",
                &quality.to_string(),
                &thumb_str,
                "-y",
            ])
            .output()
    })
    .await
    .map_err(|_| AppError::Internal)?
    .map_err(|_| AppError::Internal)?;

    if output.status.success() && thumb_path.exists() {
        Ok(Some(thumb_path))
    } else {
        if !output.status.success() {
            warn!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr));
        }
        Ok(None)
    }
}

async fn probe_video_stream(path: &StdPath, config: &Config) -> bool {
    let ffprobe = config
        .ffmpeg_path
        .rsplit_once(['/', '\\'])
        .map(|(dir, _)| format!("{dir}/ffprobe"))
        .unwrap_or_else(|| "ffprobe".to_string());
    let path_str = path.to_string_lossy().into_owned();

    tokio::task::spawn_blocking(move || {
        let out = std::process::Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                &path_str,
            ])
            .output();

        match out {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .trim()
                .eq_ignore_ascii_case("video"),
            Ok(out) => {
                warn!(
                    "ffprobe failed for {}: {}",
                    path_str,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                false
            }
            Err(e) => {
                warn!("ffprobe invocation error: {e}");
                false
            }
        }
    })
    .await
    .unwrap_or(false)
}

async fn pdf_thumb(path: &StdPath, config: &Config, ctx: ThumbCtx) -> AppResult<Option<PathBuf>> {
    let path_buf = path.to_path_buf();
    let pdftoppm = config.pdftoppm_path.clone();
    let dpi = config.pdf_thumbnail_dpi;

    let result = tokio::task::spawn_blocking(move || {
        let prefix = path_buf.with_extension("pdfthumb.tmp");
        let prefix_str = prefix.to_string_lossy().into_owned();
        let file_str = path_buf.to_string_lossy().into_owned();

        let out = std::process::Command::new(&pdftoppm)
            .args([
                "-png",
                "-r",
                &dpi.to_string(),
                "-f",
                "1",
                "-l",
                "1",
                &file_str,
                &prefix_str,
            ])
            .output()
            .ok()?;

        if !out.status.success() {
            warn!(
                "pdftoppm failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return None;
        }

        let generated = path_buf.with_extension("pdfthumb.tmp-1.png");
        if !generated.exists() {
            warn!(
                "pdftoppm did not produce expected output: {}",
                generated.display()
            );
            return None;
        }

        let data = std::fs::read(&generated).ok()?;
        let _ = std::fs::remove_file(&generated);

        let img = image::load_from_memory(&data).ok()?;
        let new_w = ((img.width() as f64 * ctx.scale) as u32).max(1);
        let new_h = ((img.height() as f64 * ctx.scale) as u32).max(1);
        let thumb = img.resize(new_w, new_h, FilterType::Lanczos3);

        save_thumb(&thumb, &path_buf, &ctx.format, ctx.quality, &ctx.suffix)
    })
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(result)
}

async fn audio_thumb(path: &StdPath, mime_type: &str, ctx: ThumbCtx) -> AppResult<Option<PathBuf>> {
    let file_types: Vec<FileType> = match mime_type {
        "audio/mpeg" => vec![FileType::Mpeg],
        "audio/mp4" | "audio/x-m4a" => vec![FileType::Mp4],
        "audio/flac" => vec![FileType::Flac],
        "audio/ogg" => vec![FileType::Vorbis, FileType::Opus],
        "audio/opus" => vec![FileType::Opus, FileType::Vorbis],
        "audio/wav" | "audio/x-wav" => vec![FileType::Wav],
        "audio/aiff" | "audio/x-aiff" => vec![FileType::Aiff],
        _ => {
            info!("No tag reader mapped for audio mime: {mime_type}");
            return Ok(None);
        }
    };

    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        for ft in file_types {
            if let Some(p) =
                read_cover_art(&path, ft, &ctx.format, ctx.quality, &ctx.suffix, ctx.scale)
            {
                return Some(p);
            }
        }
        None
    })
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(result)
}

async fn font_thumb(path: &StdPath, config: &Config, ctx: ThumbCtx) -> AppResult<Option<PathBuf>> {
    let path = path.to_path_buf();
    let text = config.font_thumbnail_text.clone();
    let bg = parse_hex_color(&config.font_thumbnail_bg);
    let fg = parse_hex_color(&config.font_thumbnail_fg);

    let result = tokio::task::spawn_blocking(move || {
        let data = std::fs::read(&path).ok()?;
        let font = FontRef::try_from_slice(&data).ok()?;

        let base_size = 200.0f32;
        let px_scale = PxScale::from(base_size);
        let scaled = font.as_scaled(px_scale);

        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let line_gap = scaled.line_gap();
        let line_height = ascent - descent + line_gap;

        let mut width = 0.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for c in text.chars() {
            let glyph_id = scaled.glyph_id(c);
            if let Some(p) = prev {
                width += scaled.kern(p, glyph_id);
            }
            width += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }

        let img_w = (width.ceil() as u32).max(10);
        let img_h = (line_height.ceil() as u32).max(10);

        let bg_color = bg.unwrap_or(image::Rgb([255, 255, 255]));
        let fg_color = fg.unwrap_or(image::Rgb([0, 0, 0]));

        let mut canvas = image::RgbImage::from_pixel(img_w, img_h, bg_color);

        let baseline_y = ascent;
        let mut x = 0.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;

        for c in text.chars() {
            let glyph_id = scaled.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, glyph_id);
            }

            let positioned =
                glyph_id.with_scale_and_position(px_scale, ab_glyph::point(x, baseline_y));
            if let Some(outlined) = font.outline_glyph(positioned) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 || px as u32 >= img_w || py as u32 >= img_h {
                        return;
                    }
                    let c = coverage.clamp(0.0, 1.0);
                    let dst = canvas.get_pixel_mut(px as u32, py as u32);
                    let blend = |f: u8, d: u8| (f as f32 * c + d as f32 * (1.0 - c)) as u8;
                    *dst = image::Rgb([
                        blend(fg_color[0], dst[0]),
                        blend(fg_color[1], dst[1]),
                        blend(fg_color[2], dst[2]),
                    ]);
                });
            }

            x += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }

        let img = image::DynamicImage::ImageRgb8(canvas);
        let new_w = ((img.width() as f64 * ctx.scale) as u32).max(1);
        let new_h = ((img.height() as f64 * ctx.scale) as u32).max(1);
        let thumb = img.resize(new_w, new_h, FilterType::Lanczos3);

        save_thumb(&thumb, &path, &ctx.format, ctx.quality, &ctx.suffix)
    })
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(result)
}

use crate::db::conversation_lock::PendingFaceUpload;

async fn handle_face_lock_upload(
    state: Arc<AppState>,
    token: String,
    pending: PendingFaceUpload,
    mut multipart: Multipart,
) -> AppResult<Response> {
    if !state.face_detector.lock().await.is_enabled() {
        warn!("face-lock upload rejected: detector disabled");
        return Err(AppError::Internal);
    }

    let field = multipart
        .next_field()
        .await
        .map_err(|_| AppError::BadRequest("Failed to read field".into()))?
        .ok_or_else(|| AppError::BadRequest("No file provided".into()))?;

    let temp_path = stream_to_temp(&state, &token, field).await?;

    let detected_mime = detect_mime(&temp_path)
        .await?
        .ok_or_else(|| AppError::BadRequest("Unable to detect file MIME type".into()))?;

    if !detected_mime.starts_with("image/") {
        warn!("face-lock upload rejected: detected MIME {detected_mime} is not an image");
        let _ = remove_file(&temp_path).await;
        return Err(AppError::UnsupportedMediaType);
    }

    let embedding = {
        let mut det = state.face_detector.lock().await;
        match det.embed_file(&temp_path) {
            Ok(e) => e,
            Err(e) => {
                error!("face-lock upload: embedding failed: {e}");
                let _ = remove_file(&temp_path).await;
                return Err(AppError::Internal);
            }
        }
    };

    let _ = remove_file(&temp_path).await;

    if pending.slot == SLOT_PROBE {
        handle_face_lock_probe(&state, pending, embedding).await;
    } else if (0..3).contains(&pending.slot) {
        let slot = pending.slot;
        let owner_id = pending.owner_id;
        let partner_id = pending.partner_id;
        if let Err(e) = state
            .db
            .call(move |d| d.store_lock_ref_embedding(owner_id, partner_id, slot, &embedding))
            .await
        {
            error!("face-lock upload: store ref embedding failed: {e}");
            return Err(AppError::Internal);
        }
        info!(
            "face-lock ref stored for owner={} partner={} slot={}",
            pending.owner_id, pending.partner_id, slot
        );
    } else {
        warn!("face-lock upload: unknown slot {}", pending.slot);
        return Err(AppError::BadRequest("Unknown face-lock slot".into()));
    }

    Ok(axum::Json(serde_json::json!({"status": "success"})).into_response())
}

async fn handle_face_lock_probe(
    state: &Arc<AppState>,
    pending: PendingFaceUpload,
    probe: Vec<f32>,
) {
    let owner_id = pending.owner_id;
    let partner_id = pending.partner_id;

    let lock = match state
        .db
        .call(move |d| d.get_conversation_lock(owner_id, partner_id))
        .await
    {
        Ok(Some(l)) => l,
        Ok(None) => {
            warn!("face-lock probe: no lock for owner={owner_id} partner={partner_id}");
            return;
        }
        Err(e) => {
            error!("face-lock probe: lock lookup failed: {e}");
            return;
        }
    };

    let refs = lock.present_embeddings();
    let verdict = match distance_to_prototype(&probe, &refs) {
        Some(dist) => {
            let v = classify(dist, state.config.face_verify_threshold);
            info!(
                "face-lock probe: distance={dist:.4}, threshold={:.4}, verdict={:?}",
                state.config.face_verify_threshold, v
            );
            v
        }
        None => {
            warn!("face-lock probe: no reference embeddings; falling back to diffrent");
            Verdict::Diffrent
        }
    };

    if verdict == Verdict::Match {
        let (owner_name, partner_name) = match (
            state.db.call(move |d| d.get_username_by_id(owner_id)).await,
            state
                .db
                .call(move |d| d.get_username_by_id(partner_id))
                .await,
        ) {
            (Ok(o), Ok(p)) => (o, p),
            _ => {
                error!("face-lock probe: username lookup failed");
                return;
            }
        };
        state
            .online_users
            .write()
            .await
            .mark_verified(&owner_name, &partner_name);
        info!("face-lock probe: marked {owner_name} -> {partner_name} verified");
    }

    push_face_detection_response(state, owner_id, partner_id, verdict).await;
}

async fn push_face_detection_response(
    state: &Arc<AppState>,
    owner_id: i64,
    partner_id: i64,
    verdict: Verdict,
) {
    let (owner_name, partner_name) = match (
        state.db.call(move |d| d.get_username_by_id(owner_id)).await,
        state
            .db
            .call(move |d| d.get_username_by_id(partner_id))
            .await,
    ) {
        (Ok(owner), Ok(partner)) => (owner, partner),
        _ => {
            error!("push_face_detection_response: username lookup failed");
            return;
        }
    };

    let sinks = {
        let online = state.online_users.read().await;
        online.get_senders(&owner_name)
    };

    if sinks.is_empty() {
        warn!("push_face_detection_response: user {owner_name} is offline");
        return;
    }

    let payload = FaceDetectionResponse {
        msg_type: "face_detection_response".to_string(),
        status: verdict.as_status().to_string(),
        with: partner_name,
    };

    let debug = state.config.debug;

    for sink in sinks {
        let payload = payload.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::server::send_response(&sink, &payload, debug).await {
                error!("Failed to push face_detection_response: {e}");
            }
        });
    }
}
