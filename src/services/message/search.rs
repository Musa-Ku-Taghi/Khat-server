use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::{Database, DbService};
use crate::models::{ResponseStatus, SearchUserResponse, UserSearchResult};
use crate::online::OnlineUsers;

pub async fn search_user(
    db: Arc<Database>,
    online_users: Arc<RwLock<OnlineUsers>>,
    current_user: String,
    username: String,
) -> SearchUserResponse {
    let current_user_id = match {
        let current = current_user.clone();
        db.call(move |d| d.get_user_id_by_username(&current)).await
    } {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to resolve current user '{current_user}': {e}");
            return SearchUserResponse {
                msg_type: "search_user_response".to_string(),
                status: ResponseStatus::Error,
                results: vec![],
            };
        }
    };

    let result = db
        .call({
            let search = username.clone();
            move |d| d.search_users(&search)
        })
        .await;

    match result {
        Ok(results) => {
            let locked_partners =
                crate::services::message::locked_partner_names(&db, current_user_id).await;

            let online = online_users.read().await;

            let results_with_status: Vec<UserSearchResult> = results
                .into_iter()
                .map(|(uname, primary_file_id)| UserSearchResult {
                    online: online.is_online(&uname),
                    locked: locked_partners.contains(&uname)
                        && !online.is_verified(&current_user, &uname),
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
