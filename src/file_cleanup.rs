use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use tokio::fs::remove_file;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::db::{Database, DbService, FileEntry, TempFileEntry};

pub async fn cleanup_task(db: Arc<Database>, interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    loop {
        ticker.tick().await;
        info!("Running file cleanup...");

        remove_expired_temp_files(&db).await;
        remove_orphan_files(&db).await;

        info!("File cleanup finished.");
    }
}

async fn remove_expired_temp_files(db: &Arc<Database>) {
    let expired: Vec<TempFileEntry> = match db.call(|d| d.get_expired_temp_files()).await {
        Ok(files) => files,
        Err(e) => {
            error!("Failed to get expired temp files: {e}");
            return;
        }
    };

    for meta in expired {
        delete_path(&meta.storage_path).await;
        if let Some(thumb) = &meta.thumb_path {
            delete_path(thumb).await;
        }

        let id = meta.id;
        if let Err(e) = db.call(move |d| d.delete_file_metadata(id)).await {
            error!("Failed to delete temp metadata {id}: {e}");
        }
        info!("Deleted expired temp file id {}", meta.id);
    }
}

async fn remove_orphan_files(db: &Arc<Database>) {
    let referenced: HashSet<i64> = match db.call(|d| d.get_all_referenced_file_ids()).await {
        Ok(ids) => ids.into_iter().collect(),
        Err(e) => {
            error!("Failed to get referenced file IDs: {e}");
            return;
        }
    };

    let all_files: Vec<FileEntry> = match db.call(|d| d.get_all_non_temp_files()).await {
        Ok(files) => files,
        Err(e) => {
            error!("Failed to get all non-temp files: {e}");
            return;
        }
    };

    for meta in all_files {
        if referenced.contains(&meta.id) {
            continue;
        }

        info!("Deleting orphan file id {} (hash {})", meta.id, meta.hash);
        delete_path(&meta.storage_path).await;
        if let Some(thumb) = &meta.thumb_path {
            delete_path(thumb).await;
        }

        let id = meta.id;
        if let Err(e) = db.call(move |d| d.delete_file_metadata(id)).await {
            error!("Failed to delete orphan metadata {id}: {e}");
        }
    }
}

async fn delete_path(path: &str) {
    let p = Path::new(path);
    if !p.exists() {
        return;
    }
    if let Err(e) = remove_file(p).await {
        warn!("Failed to delete file {path}: {e}");
    }
}
