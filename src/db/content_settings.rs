use rusqlite::params;

use crate::db::{Database, DbError};

#[derive(Debug, Clone, Copy, Default)]
pub struct ContentSettings {
    pub spam: bool,
    pub obscene: bool,
    pub hate: bool,
}

impl ContentSettings {
    pub fn any_enabled(&self) -> bool {
        self.spam || self.obscene || self.hate
    }

    pub fn blocks_label(&self, label: &str) -> bool {
        match label {
            "spam" => self.spam,
            "obscene" => self.obscene,
            "hate" => self.hate,
            _ => false,
        }
    }
}

impl Database {
    pub fn get_content_settings(&self, user_id: i64) -> Result<ContentSettings, DbError> {
        let conn = self.get_conn()?;
        let row = conn.query_row(
            "SELECT spam, obscene, hate FROM content_analysis_settings WHERE user_id = ?1",
            [user_id],
            |row| {
                Ok(ContentSettings {
                    spam: row.get(0)?,
                    obscene: row.get(1)?,
                    hate: row.get(2)?,
                })
            },
        );

        match row {
            Ok(s) => Ok(s),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ContentSettings::default()),
            Err(e) => Err(DbError::from(e)),
        }
    }

    pub fn set_content_settings(
        &self,
        user_id: i64,
        settings: ContentSettings,
    ) -> Result<(), DbError> {
        let conn = self.get_conn()?;
        conn.execute(
            "INSERT INTO content_analysis_settings (user_id, spam, obscene, hate)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                 spam = excluded.spam,
                 obscene = excluded.obscene,
                 hate = excluded.hate",
            params![user_id, settings.spam, settings.obscene, settings.hate],
        )?;
        Ok(())
    }
}
