use bcrypt::{hash, verify};
use rusqlite::params;

use crate::db::{validate_input, Database, DbError};

impl Database {
    pub fn signup(&self, iccid: &str, username: &str, password: &str) -> Result<(), DbError> {
        validate_input(iccid, self.config.max_iccid_length, "iccid")?;
        validate_input(username, self.config.max_username_length, "username")?;
        self.validate_password(password)?;

        let iccid_hash = self.hash_iccid(iccid);
        let conn = self.get_conn()?;

        let username_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE username = ?1)",
            [username],
            |row| row.get(0),
        )?;
        if username_exists {
            return Err(DbError::UsernameExists);
        }

        let iccid_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE iccid = ?1)",
            [&iccid_hash],
            |row| row.get(0),
        )?;
        if iccid_exists {
            return Err(DbError::IccidExists);
        }

        let password_hash = hash(password, self.config.bcrypt_cost)?;
        conn.execute(
            "INSERT INTO users (iccid, username, password_hash) VALUES (?1, ?2, ?3)",
            params![iccid_hash, username, password_hash],
        )?;
        Ok(())
    }

    pub fn login(&self, username: &str, password: &str) -> Result<(), DbError> {
        validate_input(username, self.config.max_username_length, "username")?;
        self.validate_password(password)?;

        let conn = self.get_conn()?;
        let password_hash: String = conn
            .query_row(
                "SELECT password_hash FROM users WHERE username = ?1",
                [username],
                |row| row.get(0),
            )
            .map_err(|_| DbError::UsernameNotFound)?;

        if verify(password, &password_hash)? {
            Ok(())
        } else {
            Err(DbError::PasswordIncorrect)
        }
    }

    pub fn search_users(&self, search_term: &str) -> Result<Vec<(String, Option<i64>)>, DbError> {
        validate_input(search_term, self.config.max_username_length, "search_term")?;

        let search_term_lower = search_term.to_lowercase();
        let pattern = format!("%{search_term_lower}%");
        let limit = self.config.search_results_limit as i64;

        let conn = self.get_conn()?;
        let mut stmt = conn.prepare(
            "SELECT u.username, (
                 SELECT pp.file_id FROM profile_pictures pp
                 WHERE pp.user_id = u.id AND pp.is_primary = 1
                 LIMIT 1
             ) AS primary_file_id
             FROM users u
             WHERE LOWER(u.username) LIKE ?1
             ORDER BY INSTR(LOWER(u.username), ?2) ASC, u.username ASC
             LIMIT ?3",
        )?;

        let rows = stmt
            .query_map(params![pattern, search_term_lower, limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn get_user_id_by_username(&self, username: &str) -> Result<i64, DbError> {
        let conn = self.get_conn()?;
        conn.query_row(
            "SELECT id FROM users WHERE username = ?1",
            [username],
            |row| row.get(0),
        )
        .map_err(|_| DbError::UsernameNotFound)
    }

    pub fn get_username_by_id(&self, user_id: i64) -> Result<String, DbError> {
        let conn = self.get_conn()?;
        conn.query_row(
            "SELECT username FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .map_err(|_| DbError::UsernameNotFound)
    }
}
