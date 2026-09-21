use chrono::NaiveDateTime;
use rusqlite::params;

use crate::db::{Database, DbError, FileEntry, FileMetadata, TempFileEntry};
use crate::models::UploadType;

const FILE_METADATA_COLUMNS: &str = concat!(
    "id, hash, original_name, mime_type, size, storage_path, thumb_path, ",
    "created_at, is_temp, temp_expires_at, referenced_count, requester_ip, user_id, upload_type",
);

fn read_file_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileMetadata> {
    Ok(FileMetadata {
        id: row.get(0)?,
        hash: row.get(1)?,
        original_name: row.get(2)?,
        mime_type: row.get(3)?,
        size: row.get(4)?,
        storage_path: row.get(5)?,
        thumb_path: row.get(6)?,
        created_at: row.get(7)?,
        is_temp: row.get(8)?,
        temp_expires_at: row.get(9)?,
        referenced_count: row.get(10)?,
        requester_ip: row.get(11)?,
        user_id: row.get(12)?,
        upload_type: row.get(13)?,
    })
}

impl Database {
    pub fn create_temp_upload(
        &self,
        hash: &str,
        original_name: &str,
        mime_type: &str,
        size: u64,
        token: &str,
        expires_at: NaiveDateTime,
        user_id: Option<i64>,
        upload_type: UploadType,
        temp_dir: &str,
        requester_ip: &str,
    ) -> Result<i64, DbError> {
        let conn = self.get_conn()?;

        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_metadata WHERE hash = ?1)",
            [hash],
            |row| row.get(0),
        )?;
        if exists {
            return Err(DbError::FileHashExists);
        }

        let storage_path = format!("{temp_dir}/{token}");
        let expires_at_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();
        let upload_type_str = upload_type.to_string();

        conn.execute(
            "INSERT INTO file_metadata
             (hash, original_name, mime_type, size, storage_path, thumb_path,
              is_temp, temp_expires_at, upload_token, user_id, upload_type, requester_ip)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 1, ?6, ?7, ?8, ?9, ?10)",
            params![
                hash,
                original_name,
                mime_type,
                size as i64,
                storage_path,
                expires_at_str,
                token,
                user_id,
                upload_type_str,
                requester_ip,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn get_temp_upload_by_token(&self, token: &str) -> Result<Option<FileMetadata>, DbError> {
        let conn = self.get_conn()?;
        let sql = format!(
            "SELECT {FILE_METADATA_COLUMNS}
             FROM file_metadata WHERE upload_token = ?1 AND is_temp = 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map([token], read_file_metadata)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_file_by_hash(&self, hash: &str) -> Result<Option<FileMetadata>, DbError> {
        let conn = self.get_conn()?;
        let sql = format!("SELECT {FILE_METADATA_COLUMNS} FROM file_metadata WHERE hash = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map([hash], read_file_metadata)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_file_by_id(&self, id: i64) -> Result<Option<FileMetadata>, DbError> {
        let conn = self.get_conn()?;
        let sql = format!("SELECT {FILE_METADATA_COLUMNS} FROM file_metadata WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map([id], read_file_metadata)?;
        Ok(rows.next().transpose()?)
    }

    pub fn finalize_upload(
        &self,
        id: i64,
        final_storage_path: &str,
        thumb_path: Option<&str>,
        mime_type: &str,
    ) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        conn.execute(
            "UPDATE file_metadata
             SET is_temp = 0, temp_expires_at = NULL, upload_token = NULL,
                 storage_path = ?1, thumb_path = ?2, mime_type = ?3
             WHERE id = ?4",
            params![final_storage_path, thumb_path, mime_type, id],
        )?;
        Ok(())
    }

    pub fn delete_file_metadata(&self, id: i64) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        conn.execute("DELETE FROM file_metadata WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn get_expired_temp_files(&self) -> Result<Vec<TempFileEntry>, DbError> {
        let conn = self.get_conn()?;
        let now_str = chrono::Local::now()
            .naive_local()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();

        let mut stmt = conn.prepare(
            "SELECT id, storage_path, thumb_path, temp_expires_at
             FROM file_metadata WHERE is_temp = 1 AND temp_expires_at <= ?1",
        )?;
        let rows = stmt.query_map([now_str], |row| {
            Ok(TempFileEntry {
                id: row.get(0)?,
                storage_path: row.get(1)?,
                thumb_path: row.get(2)?,
                temp_expires_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_all_non_temp_files(&self) -> Result<Vec<FileEntry>, DbError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, hash, storage_path, thumb_path, mime_type, original_name, size
             FROM file_metadata WHERE is_temp = 0",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FileEntry {
                id: row.get(0)?,
                hash: row.get(1)?,
                storage_path: row.get(2)?,
                thumb_path: row.get(3)?,
                mime_type: row.get(4)?,
                original_name: row.get(5)?,
                size: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_all_referenced_file_ids(&self) -> Result<Vec<i64>, DbError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT file_id FROM file_references
             UNION
             SELECT file_id FROM profile_pictures",
        )?;
        let ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }
}
