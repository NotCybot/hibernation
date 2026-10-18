use std::time::Duration;

use centaurus::db::init::Connection;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};

use crate::db::DBTrait;

pub fn start(db: Connection) {
  tokio::spawn(async move {
    info!("Starting cache dedupe job");
    let mut interval = interval(Duration::from_mins(5));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
      interval.tick().await;
      info!("Running cache dedupe job");
      run_dedupe(&db).await;
    }
  });
}

pub async fn run_dedupe(db: &Connection) {
  let duplicates = match db.nar().find_duplicate_nars().await {
    Ok(dups) => dups,
    Err(e) => {
      warn!("Failed to find duplicate nars: {e}");
      return;
    }
  };
  info!("Found {} duplicate nars", duplicates.len());

  for hash in duplicates {
    let Ok(ids) = db.nar().find_nars_by_nar_hash(&hash).await else {
      warn!("Failed to find nars by hash {hash}");
      continue;
    };
    if ids.len() <= 1 {
      continue;
    }

    let id_to_keep = ids[0];
    let ids_to_delete = &ids[1..];
    if db
      .nar()
      .replace_nar_id(ids_to_delete, id_to_keep)
      .await
      .is_err()
    {
      warn!("Failed to replace nar ids for hash {hash}");
      continue;
    }
  }
}
