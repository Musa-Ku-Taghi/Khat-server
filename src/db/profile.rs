use rusqlite::params;

use crate::db::{Database, DbError};

#[derive(Debug, Clone)]
pub struct ProfilePicture {
    pub id: i64,
    pub file_id: i64,
    pub is_primary: bool,
    pub created_at: String,
    pub url: String,
}

impl Database {
    pub fn add_profile_picture(&self, user_id: i64, file_id: i64) -> Result<(), DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        tx.execute(
            "UPDATE profile_pictures SET is_primary = 0 WHERE user_id = ?1",
            [user_id],
        )?;

        tx.execute(
            "INSERT INTO profile_pictures (user_id, file_id, is_primary) VALUES (?1, ?2, 1)",
            params![user_id, file_id],
        )?;

        tx.execute(
            "UPDATE file_metadata SET referenced_count = referenced_count + 1 WHERE id = ?1",
            [file_id],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn remove_profile_picture(&self, user_id: i64, picture_id: i64) -> Result<(), DbError> {
        let conn = self.get_conn()?;

        let (file_id, was_primary): (i64, bool) = conn
            .query_row(
                "SELECT file_id, is_primary FROM profile_pictures
                 WHERE id = ?1 AND user_id = ?2",
                params![picture_id, user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| DbError::InvalidInput("Profile picture not found".to_string()))?;

        conn.execute(
            "DELETE FROM profile_pictures WHERE id = ?1 AND user_id = ?2",
            params![picture_id, user_id],
        )?;
        conn.execute(
            "UPDATE file_metadata SET referenced_count = referenced_count - 1 WHERE id = ?1",
            [file_id],
        )?;

        if was_primary {
            let next_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM profile_pictures
                     WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    [user_id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(next_id) = next_id {
                conn.execute(
                    "UPDATE profile_pictures SET is_primary = 1 WHERE id = ?1",
                    [next_id],
                )?;
            }
        }

        Ok(())
    }

    pub fn get_profile_pictures(&self, user_id: i64) -> Result<Vec<ProfilePicture>, DbError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT p.id, p.file_id, p.is_primary,
                    strftime('%Y-%m-%dT%H:%M:%S', p.created_at) AS created_at,
                    f.storage_path
             FROM profile_pictures p
             JOIN file_metadata f ON p.file_id = f.id
             WHERE p.user_id = ?1
             ORDER BY p.created_at DESC",
        )?;
        let rows = stmt.query_map([user_id], |row| {
            let file_id: i64 = row.get(1)?;
            Ok(ProfilePicture {
                id: row.get(0)?,
                file_id,
                is_primary: row.get(2)?,
                created_at: row.get(3)?,
                url: format!("/profile_pics/{file_id}"),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_primary_profile_picture(
        &self,
        user_id: i64,
        picture_id: i64,
    ) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM profile_pictures WHERE id = ?1 AND user_id = ?2)",
            params![picture_id, user_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(DbError::InvalidInput(
                "Profile picture not found".to_string(),
            ));
        }

        conn.execute(
            "UPDATE profile_pictures SET is_primary = 0 WHERE user_id = ?1",
            [user_id],
        )?;
        conn.execute(
            "UPDATE profile_pictures SET is_primary = 1 WHERE id = ?1",
            [picture_id],
        )?;
        Ok(())
    }

    pub fn is_profile_picture(&self, file_id: i64) -> Result<bool, DbError> {
        let conn = self.get_conn()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM profile_pictures WHERE file_id = ?1)",
            [file_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }
}
