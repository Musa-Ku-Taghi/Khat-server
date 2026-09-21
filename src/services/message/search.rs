use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::{Database, DbService};
use crate::models::{ResponseStatus, SearchUserResponse, UserSearchResult};
use crate::online::OnlineUsers;

pub async fn search_user(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    username: String,
) -> SearchUserResponse {
    let result = db
        .call({
            let search = username.clone();
            move |d| d.search_users(&search)
        })
        .await;

    match result {
        Ok(results) => {
            let online = online_users.read().await;
            let results_with_status: Vec<UserSearchResult> = results
                .into_iter()
                .map(|(uname, primary_file_id)| UserSearchResult {
                    online: online.is_online(&uname),
                    username: uname,
                    profile_picture_url: primary_file_id.map(|id| format!("/profile_pics/{id}")),
                })
                .collect();
            let online_count = results_with_status.iter().filter(|r| r.online).count();
            info!(
                "User search successful: query='{username}', found={}, online={online_count}",
                results_with_status.len()
            );
            SearchUserResponse {
                msg_type: "search_user_response".to_string(),
                status: ResponseStatus::Success,
                results: results_with_status,
            }
        }
        Err(e) => {
            error!("User search failed: {e}");
            SearchUserResponse {
                msg_type: "search_user_response".to_string(),
                status: ResponseStatus::Error,
                results: vec![],
            }
        }
    }
}
