use rusqlite::{params, params_from_iter, Error as RusqliteError};

use crate::db::{Database, DbError};
use crate::models::{ContentPart, Conversation, Message};

impl Database {
    pub fn get_message_participants(&self, message_id: i64) -> Result<(i64, i64), DbError> {
        let conn = self.get_conn()?;
        conn.query_row(
            "SELECT sender_id, recipient_id FROM direct_messages WHERE id = ?1",
            [message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| DbError::InvalidInput("Message not found".to_string()))
    }

    pub fn get_unread_offset_for_conversation(
        &self,
        current_user_id: i64,
        other_user_id: i64,
        chunk_size: u64,
    ) -> Result<Option<u64>, DbError> {
        let conn = self.get_conn()?;
        let row = conn.query_row(
            "SELECT id, timestamp FROM direct_messages
             WHERE recipient_id = ?1 AND sender_id = ?2 AND read = 0
             ORDER BY timestamp ASC, id ASC
             LIMIT 1",
            params![current_user_id, other_user_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        );

        let (unread_id, unread_ts) = match row {
            Ok(r) => r,
            Err(RusqliteError::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(DbError::from(e)),
        };

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM direct_messages
             WHERE ((sender_id = ?1 AND recipient_id = ?2)
                 OR (sender_id = ?2 AND recipient_id = ?1))
               AND (timestamp < ?3 OR (timestamp = ?3 AND id < ?4))",
            params![current_user_id, other_user_id, unread_ts, unread_id],
            |row| row.get(0),
        )?;

        Ok(Some((count as u64) / chunk_size))
    }

    pub fn save_message(
        &self,
        sender_id: i64,
        recipient_id: i64,
        content: &[ContentPart],
    ) -> Result<i64, DbError> {
        let file_ids = collect_file_ids(content);

        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        for &file_id in &file_ids {
            match self.get_file_by_id(file_id)? {
                Some(meta) if meta.is_temp => return Err(DbError::FileTempExpired),
                Some(_) => {}
                None => return Err(DbError::FileNotFound),
            }
        }

        let content_json = serde_json::to_string(content)?;
        tx.execute(
            "INSERT INTO direct_messages (sender_id, recipient_id, content) VALUES (?1, ?2, ?3)",
            params![sender_id, recipient_id, content_json],
        )?;
        let msg_id = tx.last_insert_rowid();

        for file_id in &file_ids {
            tx.execute(
                "UPDATE file_metadata SET referenced_count = referenced_count + 1 WHERE id = ?1",
                [file_id],
            )?;
            tx.execute(
                "INSERT INTO file_references (file_id, message_id) VALUES (?1, ?2)",
                params![file_id, msg_id],
            )?;
        }

        tx.commit()?;
        Ok(msg_id)
    }

    pub fn get_message_timestamp(&self, message_id: i64) -> Result<String, DbError> {
        let conn = self.get_conn()?;
        conn.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%S', timestamp)
             FROM direct_messages WHERE id = ?1",
            [message_id],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            RusqliteError::QueryReturnedNoRows => {
                DbError::InvalidInput("Message not found".to_string())
            }
            other => DbError::DatabaseError(other.to_string()),
        })
    }

    pub fn get_messages_between_paginated(
        &self,
        user1_id: i64,
        user2_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Message>, DbError> {
        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT d.id, s.username, r.username, d.content,
                    strftime('%Y-%m-%dT%H:%M:%S', d.timestamp) AS timestamp, d.read,
                    strftime('%Y-%m-%dT%H:%M:%S', d.edited_at) AS edited_at
             FROM direct_messages d
             JOIN users s ON d.sender_id = s.id
             JOIN users r ON d.recipient_id = r.id
             WHERE (d.sender_id = ?1 AND d.recipient_id = ?2)
                OR (d.sender_id = ?2 AND d.recipient_id = ?1)
             ORDER BY d.timestamp DESC
             LIMIT ?3 OFFSET ?4",
        )?;

        let rows = stmt.query_map(params![user1_id, user2_id, limit, offset], |row| {
            let content_json: String = row.get(3)?;
            let content: Vec<ContentPart> = serde_json::from_str(&content_json).map_err(|e| {
                RusqliteError::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
            })?;

            let mut enriched = Vec::with_capacity(content.len());
            for mut part in content {
                if part.r#type == "file" {
                    if let Some(file_id) = part.file_id {
                        if part.size.is_none() || part.hash.is_none() {
                            if let Ok(Some(meta)) = self.get_file_by_id(file_id) {
                                if part.size.is_none() {
                                    part.size = Some(meta.size as u64);
                                }
                                if part.hash.is_none() {
                                    part.hash = Some(meta.hash);
                                }
                            }
                        }
                    }
                }
                enriched.push(part);
            }

            Ok(Message {
                id: row.get(0)?,
                sender: row.get(1)?,
                recipient: row.get(2)?,
                content: enriched,
                timestamp: row.get(4)?,
                read: row.get(5)?,
                edited_at: row.get(6)?,
            })
        })?;

        let mut messages: Vec<Message> = rows.collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn get_conversations_for_user(&self, user_id: i64) -> Result<Vec<Conversation>, DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        let partner_ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT other FROM (
                    SELECT recipient_id AS other FROM direct_messages WHERE sender_id = ?1
                    UNION
                    SELECT sender_id AS other FROM direct_messages WHERE recipient_id = ?1
                )",
            )?;
            let ids = stmt
                .query_map([user_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let mut conversations = Vec::with_capacity(partner_ids.len());

        for partner_id in partner_ids {
            let (with_user, primary_file_id): (String, Option<i64>) = tx
                .query_row(
                    "SELECT u.username, (
                         SELECT pp.file_id FROM profile_pictures pp
                         WHERE pp.user_id = u.id AND pp.is_primary = 1
                         LIMIT 1
                     )
                     FROM users u WHERE u.id = ?1",
                    [partner_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|_| DbError::UsernameNotFound)?;

            let (last_content_json, last_ts): (String, String) = tx.query_row(
                "SELECT content, strftime('%Y-%m-%dT%H:%M:%S', timestamp)
                 FROM direct_messages
                 WHERE (sender_id = ?1 AND recipient_id = ?2)
                    OR (sender_id = ?2 AND recipient_id = ?1)
                 ORDER BY timestamp DESC LIMIT 1",
                params![user_id, partner_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;

            let last_message: Vec<ContentPart> =
                serde_json::from_str(&last_content_json).unwrap_or_default();

            let unread_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM direct_messages
                 WHERE recipient_id = ?1 AND sender_id = ?2 AND read = 0",
                params![user_id, partner_id],
                |row| row.get(0),
            )?;

            conversations.push(Conversation {
                with_user,
                last_message,
                last_timestamp: last_ts,
                unread_count,
                online: false,
                profile_picture_url: primary_file_id.map(|id| format!("/profile_pics/{id}")),
                locked: false,
            });
        }

        tx.commit()?;
        Ok(conversations)
    }

    pub fn mark_read_for_sender(
        &self,
        recipient_id: i64,
        sender_id: i64,
    ) -> Result<Vec<i64>, DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        let ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM direct_messages
                 WHERE sender_id = ?1 AND recipient_id = ?2 AND read = 0",
            )?;
            let ids = stmt
                .query_map(params![sender_id, recipient_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        if ids.is_empty() {
            tx.commit()?;
            return Ok(vec![]);
        }

        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!("UPDATE direct_messages SET read = 1 WHERE id IN ({placeholders})");
        tx.prepare(&sql)?.execute(params_from_iter(ids.iter()))?;

        tx.commit()?;
        Ok(ids)
    }

    pub fn mark_message_read(
        &self,
        message_id: i64,
        recipient_id: i64,
    ) -> Result<Option<(i64, i64)>, DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        let (sender_id, read): (i64, bool) = tx
            .query_row(
                "SELECT sender_id, read FROM direct_messages
                 WHERE id = ?1 AND recipient_id = ?2",
                params![message_id, recipient_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| match e {
                RusqliteError::QueryReturnedNoRows => {
                    DbError::InvalidInput("Message not found or not for this user".to_string())
                }
                other => DbError::DatabaseError(other.to_string()),
            })?;

        if read {
            tx.commit()?;
            return Ok(None);
        }

        let rows_updated = tx.execute(
            "UPDATE direct_messages SET read = 1 WHERE id = ?1 AND recipient_id = ?2",
            params![message_id, recipient_id],
        )?;

        tx.commit()?;

        if rows_updated == 1 {
            Ok(Some((sender_id, message_id)))
        } else {
            Ok(None)
        }
    }

    pub fn edit_message(
        &self,
        message_id: i64,
        user_id: i64,
        new_content: &[ContentPart],
    ) -> Result<String, DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        let sender_id: i64 = tx
            .query_row(
                "SELECT sender_id FROM direct_messages WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .map_err(|_| DbError::InvalidInput("Message not found".to_string()))?;

        if sender_id != user_id {
            return Err(DbError::InvalidInput(
                "Only the sender can edit this message".to_string(),
            ));
        }

        for file_id in collect_file_ids(new_content) {
            match self.get_file_by_id(file_id)? {
                Some(meta) if meta.is_temp => {
                    return Err(DbError::InvalidInput(
                        "Cannot reference a temporary file".to_string(),
                    ));
                }
                Some(_) => {}
                None => {
                    return Err(DbError::InvalidInput(format!("File {file_id} not found")));
                }
            }
        }

        let content_json = serde_json::to_string(new_content)?;
        let edited_at_str = chrono::Local::now()
            .naive_local()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();

        tx.execute(
            "UPDATE direct_messages SET content = ?1, edited_at = ?2 WHERE id = ?3",
            params![content_json, edited_at_str, message_id],
        )?;

        tx.commit()?;
        Ok(edited_at_str)
    }

    pub fn delete_message(&self, message_id: i64, user_id: i64) -> Result<(), DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        let sender_id: i64 = tx
            .query_row(
                "SELECT sender_id FROM direct_messages WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .map_err(|_| DbError::InvalidInput("Message not found".to_string()))?;

        if sender_id != user_id {
            return Err(DbError::InvalidInput(
                "Only the sender can delete this message".to_string(),
            ));
        }

        let file_ids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT file_id FROM file_references WHERE message_id = ?1")?;
            let ids = stmt
                .query_map([message_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        tx.execute(
            "DELETE FROM file_references WHERE message_id = ?1",
            [message_id],
        )?;

        for file_id in file_ids {
            tx.execute(
                "UPDATE file_metadata SET referenced_count = referenced_count - 1 WHERE id = ?1",
                [file_id],
            )?;
        }

        tx.execute("DELETE FROM direct_messages WHERE id = ?1", [message_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_message_chunk_id(&self, message_id: i64, chunk_size: u64) -> Result<u64, DbError> {
        let conn = self.get_conn()?;

        let (sender_id, recipient_id, timestamp): (i64, i64, String) = conn
            .query_row(
                "SELECT sender_id, recipient_id, timestamp
             FROM direct_messages
             WHERE id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| DbError::InvalidInput("Message not found".to_string()))?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*)
         FROM direct_messages
         WHERE ((sender_id = ?1 AND recipient_id = ?2)
             OR (sender_id = ?2 AND recipient_id = ?1))
           AND (timestamp > ?3 OR (timestamp = ?3 AND id > ?4))",
            params![sender_id, recipient_id, timestamp, message_id],
            |row| row.get(0),
        )?;

        Ok((count as u64) / chunk_size)
    }

    pub fn get_message_content(&self, message_id: i64) -> Result<Vec<ContentPart>, DbError> {
        let conn = self.get_conn()?;

        let content_json: String = conn
            .query_row(
                "SELECT content FROM direct_messages WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )
            .map_err(|_| DbError::InvalidInput("Message not found".to_string()))?;

        serde_json::from_str(&content_json).map_err(|e| DbError::DatabaseError(e.to_string()))
    }
}

fn collect_file_ids(content: &[ContentPart]) -> Vec<i64> {
    content
        .iter()
        .filter(|p| p.r#type == "file")
        .filter_map(|p| p.file_id)
        .collect()
}
