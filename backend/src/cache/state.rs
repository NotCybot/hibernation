use std::sync::Arc;

use aide::OperationIo;
use axum::{Extension, extract::FromRequestParts};
use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

#[derive(Clone, FromRequestParts, OperationIo)]
#[from_request(via(Extension))]
pub struct CacheEvictionState {
  cache_locks: Arc<DashMap<Uuid, Arc<Mutex<()>>>>,
}

impl Default for CacheEvictionState {
  fn default() -> Self {
    Self::new()
  }
}

impl CacheEvictionState {
  pub fn new() -> Self {
    Self {
      cache_locks: Arc::new(DashMap::new()),
    }
  }

  pub async fn lock_cache(&self, cache_id: Uuid) -> OwnedMutexGuard<()> {
    let mutex = self
      .cache_locks
      .entry(cache_id)
      .or_insert_with(|| Arc::new(Mutex::new(())))
      .clone();

    mutex.lock_owned().await
  }
}
