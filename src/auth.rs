use std::collections::HashMap;
use std::sync::Arc;

use rand::Rng;
use tokio::sync::Mutex;

pub type TokenMap = Arc<Mutex<HashMap<String, String>>>;

pub fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    hex::encode(rng.gen::<[u8; 16]>())
}

pub async fn store_token(tokens: &TokenMap, username: &str) -> String {
    let token = generate_token();
    let mut map = tokens.lock().await;
    map.insert(token.clone(), username.to_string());
    token
}

pub async fn validate_token(tokens: &TokenMap, token: &str) -> Option<String> {
    let map = tokens.lock().await;
    map.get(token).cloned()
}

pub async fn invalidate_token(tokens: &TokenMap, token: &str) {
    let mut map = tokens.lock().await;
    map.remove(token);
}
