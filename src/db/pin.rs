use rusqlite::params;

use crate::db::{Database, DbError};
use crate::models::ContentPart;

impl Database {
    pub fn pin_message(&self, user_id: i64, message_id: i64) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        let (sender_id, recipient_id): (i64, i64) = conn
            .query_row(
                "SELECT sender_id, recipient_id FROM direct_messages WHERE id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| DbError::InvalidInput("Message not found".to_string()))?;

        if sender_id != user_id && recipient_id != user_id {
            return Err(DbError::InvalidInput("Not a participant".to_string()));
        }

        let (user1, user2) = if sender_id < recipient_id {
            (sender_id, recipient_id)
        } else {
            (recipient_id, sender_id)
        };

        conn.execute(
            "INSERT OR IGNORE INTO pinned_messages (user1_id, user2_id, message_id)
             VALUES (?1, ?2, ?3)",
            params![user1, user2, message_id],
        )?;
        Ok(())
    }

    pub fn unpin_message(&self, user_id: i64, message_id: i64) -> Result<(), DbError> {
        let conn = self.get_conn()?;

        let (sender_id, recipient_id): (i64, i64) = conn
            .query_row(
                "SELECT sender_id, recipient_id FROM direct_messages WHERE id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| DbError::InvalidInput("Message not found".to_string()))?;

        if sender_id != user_id && recipient_id != user_id {
            return Err(DbError::InvalidInput("Not a participant".to_string()));
        }

        let (user1, user2) = if sender_id < recipient_id {
            (sender_id, recipient_id)
        } else {
            (recipient_id, sender_id)
        };

        let rows = conn.execute(
            "DELETE FROM pinned_messages
         WHERE user1_id = ?1 AND user2_id = ?2 AND message_id = ?3",
            params![user1, user2, message_id],
        )?;

        if rows == 0 {
            return Err(DbError::InvalidInput("Message is not pinned".to_string()));
        }

        Ok(())
    }

    pub fn get_pinned_messages(
        &self,
        user_id: i64,
        other_username: &str,
        chunk_size: u64,
    ) -> Result<Vec<(i64, u64, Vec<ContentPart>)>, DbError> {
        let conn = self.get_conn()?;

        let other_id: i64 = conn
            .query_row(
                "SELECT id FROM users WHERE username = ?1",
                [other_username],
                |row| row.get(0),
            )
            .map_err(|_| DbError::UsernameNotFound)?;

        let (user1, user2) = if user_id < other_id {
            (user_id, other_id)
        } else {
            (other_id, user_id)
        };

        let mut stmt = conn.prepare(
            "SELECT m.id, m.timestamp, m.content
         FROM direct_messages m
         JOIN pinned_messages p ON p.message_id = m.id
         WHERE p.user1_id = ?1 AND p.user2_id = ?2
         ORDER BY p.pinned_at ASC",
        )?;

        let rows = stmt.query_map(params![user1, user2], |row| {
            let msg_id: i64 = row.get(0)?;
            let timestamp: String = row.get(1)?;
            let content_json: String = row.get(2)?;

            let content: Vec<ContentPart> = serde_json::from_str(&content_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;

            Ok((msg_id, timestamp, content))
        })?;

        let mut pinned = Vec::new();

        for row in rows {
            let (msg_id, timestamp, content) = row?;

            let newer_count: i64 = conn.query_row(
                "SELECT COUNT(*)
             FROM direct_messages
             WHERE ((sender_id = ?1 AND recipient_id = ?2)
                 OR (sender_id = ?2 AND recipient_id = ?1))
               AND (timestamp > ?3 OR (timestamp = ?3 AND id > ?4))",
                params![user1, user2, timestamp, msg_id],
                |row| row.get(0),
            )?;

            let chunk_id = (newer_count as u64) / chunk_size;

            pinned.push((msg_id, chunk_id, content));
        }

        Ok(pinned)
    }
}
