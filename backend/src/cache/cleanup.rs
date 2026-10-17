use std::{
  io::ErrorKind,
  path::{Path, PathBuf},
  time::{Duration, SystemTime},
};

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

/// `local_path` is the storage directory when local storage is used
pub fn start(db: Connection, storage: FileStorage, local_path: Option<PathBuf>) {
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
      if let Some(path) = local_path.clone() {
        match tokio::task::spawn_blocking(move || sweep_staging_files(&path)).await {
          Ok(Ok(deleted)) => info!("Deleted {deleted} unfinished staging files"),
          Ok(Err(e)) => warn!("Staging file sweep failed: {e}"),
          Err(e) => warn!("Staging file sweep failed: {e}"),
        }
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
  let (nars, uploads) = db.nar().known_ids().await?;

  let mut deleted = 0;
  for file in files {
    let unknown = match StoredFile::parse(file.as_ref()) {
      // Parts only belong to running uploads. A late chunk request can write one after its upload finished
      Some(StoredFile::Part(id)) => !uploads.contains(&id),
      // The final file exists before its nar entry while the upload is finished
      Some(StoredFile::Nar(id)) => !nars.contains(&id) && !uploads.contains(&id),
      None => false,
    };
    if unknown {
      storage.delete_file(&file).await?;
      deleted += 1;
    }
  }
  info!("Deleted {deleted} unknown files");

  Ok(())
}

/// Local storage writes to `<path>#<n>` first and renames when done. A crash mid-write leaves these behind and
/// they are hidden from `list`, so they are removed by walking the directory instead.
fn sweep_staging_files(dir: &Path) -> std::io::Result<usize> {
  let cut_off = SystemTime::now() - Duration::from_hours(1);
  let mut deleted = 0;
  // Files and directories vanish while uploads are running, that is not an error
  let entries = match std::fs::read_dir(dir) {
    Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
    entries => entries?,
  };
  for entry in entries {
    let res = entry.and_then(|entry| {
      let meta = entry.metadata()?;
      if meta.is_dir() {
        let deleted = sweep_staging_files(&entry.path())?;
        // Only succeeds for empty directories, e.g. the parts directory of a staging file removed above
        let _ = std::fs::remove_dir(entry.path());
        Ok(deleted)
      } else if entry.file_name().to_string_lossy().contains('#') && meta.modified()? < cut_off {
        std::fs::remove_file(entry.path())?;
        Ok(1)
      } else {
        Ok(0)
      }
    });
    match res {
      Ok(n) => deleted += n,
      Err(e) if e.kind() == ErrorKind::NotFound => {}
      Err(e) => return Err(e),
    }
  }
  Ok(deleted)
}

#[derive(Debug, PartialEq)]
enum StoredFile {
  /// `{id}.nar`
  Nar(Uuid),
  /// `parts/{id}/{part}`
  Part(Uuid),
}

impl StoredFile {
  fn parse(path: &str) -> Option<Self> {
    if let Some(id) = path.strip_suffix(".nar") {
      return Uuid::parse_str(id).ok().map(Self::Nar);
    }
    let id = path.strip_prefix("parts/")?.split('/').next()?;
    Uuid::parse_str(id).ok().map(Self::Part)
  }
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
    // Row first: if deleting the file fails afterwards the storage sweep removes it
    match db.nar().delete_orphan_nar(orphan.id).await {
      Ok(true) => {
        if let Err(e) = storage
          .delete_file(&format!("{}.nar", orphan.id).into())
          .await
        {
          warn!("Failed to delete orphan nar file {}: {e}", orphan.id);
        }
      }
      Ok(false) => {}
      Err(e) => warn!("Failed to delete orphan nar {} from db: {e}", orphan.id),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_file_ids() {
    let id = Uuid::new_v4();
    assert_eq!(
      StoredFile::parse(&format!("{id}.nar")),
      Some(StoredFile::Nar(id))
    );
    assert_eq!(
      StoredFile::parse(&format!("parts/{id}/3")),
      Some(StoredFile::Part(id))
    );
    assert_eq!(StoredFile::parse("parts/nope/3"), None);
    assert_eq!(StoredFile::parse("other.txt"), None);
    assert_eq!(StoredFile::parse(&format!("{id}.narinfo")), None);
  }

  #[test]
  fn sweeps_old_staging_files() {
    let dir = std::env::temp_dir().join(format!("hibernation-sweep-{}", Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("parts/x")).unwrap();
    let old = SystemTime::now() - Duration::from_hours(2);
    for name in ["a.nar#1", "parts/x/0#2", "b.nar"] {
      let file = std::fs::File::create(dir.join(name)).unwrap();
      file.set_modified(old).unwrap();
    }
    std::fs::write(dir.join("c.nar#3"), b"").unwrap();

    assert_eq!(sweep_staging_files(&dir).unwrap(), 2);
    assert!(dir.join("b.nar").exists());
    assert!(dir.join("c.nar#3").exists());
    assert!(!dir.join("a.nar#1").exists());
    assert!(!dir.join("parts/x").exists());
    assert_eq!(sweep_staging_files(&dir.join("missing")).unwrap(), 0);
    let _ = std::fs::remove_dir_all(dir);
  }
}
