use rusqlite::{params, OptionalExtension};

use crate::db::{Database, DbError};

pub const SLOT_PROBE: i32 = -1;

#[derive(Debug, Clone)]
pub struct ConversationLock {
    pub owner_id: i64,
    pub partner_id: i64,
    pub hashes: [String; 3],

    pub embeddings: [Option<Vec<f32>>; 3],
}

impl ConversationLock {
    pub fn is_armed(&self) -> bool {
        self.embeddings.iter().all(|e| e.is_some())
    }

    pub fn present_embeddings(&self) -> Vec<Vec<f32>> {
        self.embeddings.iter().flatten().cloned().collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PendingFaceUpload {
    pub owner_id: i64,
    pub partner_id: i64,

    pub slot: i32,
}

impl Database {
    pub fn get_conversation_lock(
        &self,
        owner_id: i64,
        partner_id: i64,
    ) -> Result<Option<ConversationLock>, DbError> {
        let conn = self.get_conn()?;
        let row: Option<(i64, String, String, String)> = conn
            .query_row(
                "SELECT id, hash_0, hash_1, hash_2
                 FROM conversation_locks
                 WHERE owner_id = ?1 AND partner_id = ?2",
                params![owner_id, partner_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        let Some((lock_id, h0, h1, h2)) = row else {
            return Ok(None);
        };

        let mut embeddings: [Option<Vec<f32>>; 3] = [None, None, None];
        let mut stmt =
            conn.prepare("SELECT slot, embedding FROM conversation_lock_refs WHERE lock_id = ?1")?;
        let rows = stmt.query_map([lock_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for r in rows {
            let (slot, blob) = r?;
            let slot_idx = slot as usize;
            if slot_idx < 3 && blob.len() == 512 * 4 {
                embeddings[slot_idx] = Some(blob_to_f32(&blob));
            }
        }

        Ok(Some(ConversationLock {
            owner_id,
            partner_id,
            hashes: [h0, h1, h2],
            embeddings,
        }))
    }

    pub fn get_pending_face_upload(
        &self,
        token: &str,
    ) -> Result<Option<PendingFaceUpload>, DbError> {
        let conn = self.get_conn()?;

        conn.query_row(
            "SELECT owner_id, partner_id, slot
         FROM face_lock_pending_uploads
         WHERE token = ?1",
            [token],
            |row| {
                Ok(PendingFaceUpload {
                    owner_id: row.get(0)?,
                    partner_id: row.get(1)?,
                    slot: row.get::<_, i64>(2)? as i32,
                })
            },
        )
        .optional()
        .map_err(DbError::from)
    }

    pub fn delete_pending_face_upload(&self, token: &str) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        conn.execute(
            "DELETE FROM face_lock_pending_uploads WHERE token = ?1",
            [token],
        )?;
        Ok(())
    }

    pub fn create_conversation_lock(
        &self,
        owner_id: i64,
        partner_id: i64,
        hashes: &[String; 3],
    ) -> Result<i64, DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        tx.execute(
            "DELETE FROM conversation_locks WHERE owner_id = ?1 AND partner_id = ?2",
            params![owner_id, partner_id],
        )?;
        tx.execute(
            "INSERT INTO conversation_locks (owner_id, partner_id, hash_0, hash_1, hash_2)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![owner_id, partner_id, hashes[0], hashes[1], hashes[2]],
        )?;
        let lock_id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(lock_id)
    }

    pub fn store_lock_ref_embedding(
        &self,
        owner_id: i64,
        partner_id: i64,
        slot: i32,
        embedding: &[f32],
    ) -> Result<(), DbError> {
        if !(0..3).contains(&slot) {
            return Err(DbError::InvalidInput(format!(
                "invalid reference slot: {slot}"
            )));
        }
        if embedding.len() != 512 {
            return Err(DbError::InvalidInput(format!(
                "embedding must be 512-d, got {}",
                embedding.len()
            )));
        }

        let conn = self.get_conn()?;
        let lock_id: i64 = conn
            .query_row(
                "SELECT id FROM conversation_locks WHERE owner_id = ?1 AND partner_id = ?2",
                params![owner_id, partner_id],
                |row| row.get(0),
            )
            .map_err(|_| DbError::InvalidInput("chat lock not found".into()))?;

        conn.execute(
            "INSERT INTO conversation_lock_refs (lock_id, slot, embedding)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(lock_id, slot) DO UPDATE SET embedding = excluded.embedding",
            params![lock_id, slot, f32_to_blob(embedding)],
        )?;
        Ok(())
    }

    pub fn register_pending_face_upload(
        &self,
        token: &str,
        owner_id: i64,
        partner_id: i64,
        slot: i32,
    ) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        conn.execute(
            "INSERT INTO face_lock_pending_uploads (token, owner_id, partner_id, slot)
             VALUES (?1, ?2, ?3, ?4)",
            params![token, owner_id, partner_id, slot],
        )?;
        Ok(())
    }

    pub fn take_pending_face_upload(
        &self,
        token: &str,
    ) -> Result<Option<PendingFaceUpload>, DbError> {
        let mut conn = self.get_conn()?;
        let tx = conn.transaction()?;

        let row: Option<(i64, i64, i64)> = tx
            .query_row(
                "SELECT owner_id, partner_id, slot
                 FROM face_lock_pending_uploads
                 WHERE token = ?1",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let Some((owner_id, partner_id, slot)) = row else {
            tx.commit()?;
            return Ok(None);
        };

        tx.execute(
            "DELETE FROM face_lock_pending_uploads WHERE token = ?1",
            [token],
        )?;
        tx.commit()?;

        Ok(Some(PendingFaceUpload {
            owner_id,
            partner_id,
            slot: slot as i32,
        }))
    }

    pub fn get_locks_owned_by(&self, user_id: i64) -> Result<Vec<ConversationLock>, DbError> {
        let conn = self.get_conn()?;
        let mut stmt =
            conn.prepare("SELECT partner_id FROM conversation_locks WHERE owner_id = ?1")?;
        let partners: Vec<i64> = stmt
            .query_map([user_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);

        let mut out = Vec::with_capacity(partners.len());
        for partner in partners {
            if let Some(lock) = self.get_conversation_lock(user_id, partner)? {
                out.push(lock);
            }
        }
        Ok(out)
    }
}

fn f32_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn blob_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
