use std::sync::Arc;

use crate::db::{Database, DbError};

pub trait DbService {
    fn call<F, T>(&self, f: F) -> impl std::future::Future<Output = Result<T, DbError>> + Send
    where
        F: FnOnce(&Database) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static;
}

impl DbService for Arc<Database> {
    async fn call<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Database) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.clone();
        tokio::task::spawn_blocking(move || f(&db)).await?
    }
}
