use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use futures_util::stream::SplitSink;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::server::WsStream;

pub struct OnlineUsers {
    senders: DashMap<String, Vec<Arc<Mutex<SplitSink<WsStream, Message>>>>>,

    verified_locks: DashMap<String, DashSet<String>>,
}

impl OnlineUsers {
    pub fn new() -> Self {
        Self {
            senders: DashMap::new(),
            verified_locks: DashMap::new(),
        }
    }

    pub fn is_online(&self, username: &str) -> bool {
        self.senders.get(username).is_some_and(|v| !v.is_empty())
    }

    pub fn add_sender(&self, username: String, sender: Arc<Mutex<SplitSink<WsStream, Message>>>) {
        self.senders.entry(username).or_default().push(sender);
    }

    pub fn remove_sender(
        &self,
        username: &str,
        sender: &Arc<Mutex<SplitSink<WsStream, Message>>>,
    ) -> bool {
        let Some(mut vec) = self.senders.get_mut(username) else {
            return false;
        };
        vec.retain(|s| !Arc::ptr_eq(s, sender));
        if vec.is_empty() {
            drop(vec);
            self.senders.remove(username);
            false
        } else {
            true
        }
    }

    pub fn get_senders(&self, username: &str) -> Vec<Arc<Mutex<SplitSink<WsStream, Message>>>> {
        self.senders
            .get(username)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn mark_verified(&self, owner: &str, partner: &str) {
        self.verified_locks
            .entry(owner.to_string())
            .or_default()
            .insert(partner.to_string());
    }

    pub fn is_verified(&self, owner: &str, partner: &str) -> bool {
        self.verified_locks
            .get(owner)
            .is_some_and(|s| s.contains(partner))
    }

    pub fn clear_verified(&self, owner: &str) {
        self.verified_locks.remove(owner);
    }
}

impl Default for OnlineUsers {
    fn default() -> Self {
        Self::new()
    }
}
