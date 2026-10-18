use std::{
  hash::Hash,
  time::{Duration, Instant},
};

use centaurus::{
  UpdateMessage,
  backend::{
    auth::permission::{self, Permission},
    endpoints::websocket,
  },
  permission,
};
use dashmap::DashMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const LOOKUP_TTL: Duration = Duration::from_secs(30);

pub fn fresh<K: Eq + Hash, V: Clone>(map: &DashMap<K, (Instant, V)>, key: &K) -> Option<V> {
  map
    .get(key)
    .filter(|entry| entry.0.elapsed() < LOOKUP_TTL)
    .map(|entry| entry.1.clone())
}

pub type Updater = websocket::state::Updater<UpdateMessage>;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, UpdateMessage)]
#[serde(tag = "type")]
pub enum UpdateMessage {
  #[update_message(settings)]
  Settings,
  #[update_message(user)]
  User {
    uuid: Uuid,
  },
  #[update_message(user_permissions)]
  UserPermissions,
  #[update_message(group)]
  Group {
    uuid: Uuid,
  },
  Token {
    uuid: Uuid,
  },
  Cache {
    uuid: Uuid,
  },
}

pub fn permissions() -> Vec<&'static str> {
  let mut perms = permission::permissions();
  perms.extend_from_slice(&[CacheCreate::name(), CacheView::name(), CacheEdit::name()]);
  perms
}

// Caches
permission!(CacheCreate, "cache:create");
permission!(CacheView, "cache:view");
permission!(CacheEdit, "cache:edit");

pub fn client() -> Client {
  Client::builder()
    .user_agent(format!("Hibernation v{}", env!("CARGO_PKG_VERSION")))
    // Used for downstream cache checks, a hanging downstream must not stall pushes
    .connect_timeout(Duration::from_secs(10))
    .timeout(Duration::from_secs(10))
    .build()
    .expect("Failed to build HTTP client")
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn fresh_expires_after_ttl() {
    let map = DashMap::new();
    map.insert("new", (Instant::now(), 1));
    map.insert("old", (Instant::now() - LOOKUP_TTL, 2));

    assert_eq!(fresh(&map, &"new"), Some(1));
    assert_eq!(fresh(&map, &"old"), None);
    assert_eq!(fresh(&map, &"missing"), None);
  }

  #[test]
  fn permissions_include_centaurus_and_cache_perms() {
    let perms = permissions();
    assert_eq!(perms.len(), permission::permissions().len() + 3);
    for perm in [
      "cache:create",
      "cache:view",
      "cache:edit",
      "user:edit",
      "group:view",
    ] {
      assert!(perms.contains(&perm), "{perm}");
    }
  }
}
