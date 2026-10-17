use std::time::Duration;

use centaurus::{
  db::init::Connection,
  error::{ErrorReportStatusExt, Result},
  storage::FileStorage,
};
use chrono::Utc;
use futures_util::TryStreamExt;
use http::StatusCode;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{cache::push::cleanup_upload, db::DBTrait};

pub fn start(db: Connection, storage: FileStorage) {
  tokio::spawn({
    let db = db.clone();
    let storage = storage.clone();
    async move {
      info!("Starting cache cleanup job");
      let mut interval = interval(Duration::from_mins(5));
      interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
      loop {
        interval.tick().await;
        info!("Running cache cleanup job");
        run_cleanup(&db, &storage).await;
      }
    }
  });

  tokio::spawn(async move {
    info!("Starting storage sweep job");
    let mut interval = interval(Duration::from_hours(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
      interval.tick().await;
      info!("Running storage sweep job");
      if let Err(e) = run_sweep(&db, &storage).await {
        warn!("Storage sweep failed: {e:?}");
      }
    }
  });
}

/// Removes files no db entry knows about, e.g. left behind by crashes or older versions
async fn run_sweep(db: &Connection, storage: &FileStorage) -> Result<()> {
  let cut_off = Utc::now() - chrono::Duration::hours(1);
  let files = storage
    .list(None)
    .try_filter(|meta| std::future::ready(meta.last_modified < cut_off))
    .map_ok(|meta| meta.location)
    .try_collect::<Vec<_>>()
    .await
    .status(StatusCode::INTERNAL_SERVER_ERROR)?;
  // Loaded after listing so entries created in the meantime are included
  let known = db.nar().known_ids().await?;

  let mut deleted = 0;
  for file in files {
    if file_id(file.as_ref()).is_some_and(|id| !known.contains(&id)) {
      storage.delete_file(&file).await?;
      deleted += 1;
    }
  }
  info!("Deleted {deleted} unknown files");

  Ok(())
}

/// Id of the nar or upload a file belongs to: `{id}.nar` or `parts/{id}/{part}`
fn file_id(path: &str) -> Option<Uuid> {
  let id = path
    .strip_suffix(".nar")
    .or_else(|| path.strip_prefix("parts/")?.split('/').next())?;
  Uuid::parse_str(id).ok()
}

async fn run_cleanup(db: &Connection, storage: &FileStorage) {
  // Runs first so files of uploads that already created their nar entry are picked up by the orphan cleanup below
  match db.nar().stale_uploads().await {
    Ok(stale) => {
      info!("Found {} stale uploads", stale.len());
      for upload in stale {
        if let Err(e) = cleanup_upload(storage, db, upload.id, upload.multipart_id.as_ref()).await {
          warn!("Failed to clean up stale upload {}: {e:?}", upload.id);
        }
      }
    }
    Err(e) => warn!("Failed to get stale uploads: {e}"),
  }

  let orphan = match db.nar().orphan_nars().await {
    Ok(orphan) => orphan,
    Err(e) => {
      warn!("Failed to get orphan nars: {e}");
      return;
    }
  };

  info!("Found {} orphan nars", orphan.len());
  for orphan in orphan {
    let nar_name = format!("{}.nar", orphan.id);
    if let Err(e) = storage.delete_file(&nar_name.into()).await {
      warn!("Failed to delete orphan nar {}: {e}", orphan.id);
    } else {
      if let Err(e) = db.nar().delete_nar(orphan.id).await {
        warn!("Failed to delete orphan nar {} from db: {e}", orphan.id);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_file_ids() {
    let id = Uuid::new_v4();
    assert_eq!(file_id(&format!("{id}.nar")), Some(id));
    assert_eq!(file_id(&format!("parts/{id}/3")), Some(id));
    assert_eq!(file_id("parts/nope/3"), None);
    assert_eq!(file_id("other.txt"), None);
    assert_eq!(file_id(&format!("{id}.narinfo")), None);
  }
}
