use std::{sync::Arc, time::Instant};

use aide::{OperationIo, axum::ApiRouter};
use axum::{
  Extension,
  body::Body,
  extract::{FromRequestParts, Path},
  routing::{any, get, head},
};
use centaurus::{bail, db::init::Connection, error::Result, storage::FileStorage};
use dashmap::DashMap;
use entity::cache;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
  auth::cli_auth::CliAuth,
  db::{DBTrait, nar::NarInfoData},
  utils::fresh,
};

/// Recent cache lookups and access checks, so bursts of narinfo requests only query the narinfo itself
#[derive(Clone, Default, FromRequestParts, OperationIo)]
#[from_request(via(Extension))]
pub struct NixCache {
  caches: Arc<DashMap<String, (Instant, cache::Model)>>,
  access: Arc<DashMap<(Uuid, Uuid), (Instant, bool)>>,
}

impl NixCache {
  pub fn clear(&self) {
    self.caches.clear();
    self.access.clear();
  }
}

/// Resolves the cache by name and checks that the request may read it
async fn readable_cache(
  db: &Connection,
  nix_cache: &NixCache,
  name: String,
  auth: Option<CliAuth>,
) -> Result<cache::Model> {
  let name = name.to_lowercase();
  let cache = match fresh(&nix_cache.caches, &name) {
    Some(cache) => cache,
    None => {
      let Some(cache) = db.cache().by_name(name.clone()).await? else {
        bail!(NOT_FOUND, "Cache not found");
      };
      nix_cache
        .caches
        .insert(name, (Instant::now(), cache.clone()));
      cache
    }
  };

  if cache.public {
    return Ok(cache);
  }

  let Some(auth) = auth else {
    bail!(UNAUTHORIZED, "Authentication required");
  };

  let key = (auth.user_id, cache.id);
  let allowed = match fresh(&nix_cache.access, &key) {
    Some(allowed) => allowed,
    None => {
      let allowed = db
        .cache()
        .cache_user_access(auth.user_id, cache.id)
        .await?
        .is_some();
      nix_cache.access.insert(key, (Instant::now(), allowed));
      allowed
    }
  };

  if !allowed {
    bail!(FORBIDDEN, "Access denied");
  }

  Ok(cache)
}

const CACHE_INFO_MIME: HeaderValue = HeaderValue::from_static("text/x-nix-cache-info");
const NAR_INFO_MIME: HeaderValue = HeaderValue::from_static("text/x-nix-narinfo");
const NAR_MIME: HeaderValue = HeaderValue::from_static("application/x-nix-nar");
const ACCEPT_RANGES: HeaderValue = HeaderValue::from_static("bytes");

/// https://fzakaria.github.io/nix-http-binary-cache-api-spec/#/default
pub fn router() -> ApiRouter {
  ApiRouter::new()
    .route("/{name}/nix-cache-info", get(nix_cache_info))
    .route("/{name}/{path}", head(head_nar_info))
    .route("/{name}/{path}", get(nar_info))
    .route("/{name}/nar/{hash}", get(nar).head(head_nar))
    // Plain 404 for endpoints nix probes but we don't serve (realisations, logs, ...), otherwise the frontend proxy
    // catch-all would answer them
    .route("/{name}", any(not_found))
    .route("/{name}/", any(not_found))
    .route("/{name}/{dir}/", any(not_found))
    .route("/{name}/{dir}/{*rest}", any(not_found))
}

async fn not_found() -> StatusCode {
  StatusCode::NOT_FOUND
}

#[derive(Deserialize, JsonSchema)]
struct CachePath {
  name: String,
}

async fn nix_cache_info(
  db: Connection,
  nix_cache: NixCache,
  Path(path): Path<CachePath>,
  auth: Option<CliAuth>,
) -> Result<(HeaderMap, String)> {
  let cache = readable_cache(&db, &nix_cache, path.name, auth).await?;

  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, CACHE_INFO_MIME.clone());

  Ok((
    headers,
    format!(
      "StoreDir: /nix/store
WantMassQuery: 1
Priority: {}
",
      cache.priority
    ),
  ))
}

#[derive(Deserialize, JsonSchema)]
struct NarInfoPath {
  name: String,
  path: String,
}

/// `<hash>.narinfo` -> `<hash>`
fn narinfo_hash(path: &str) -> Option<&str> {
  path.strip_suffix(".narinfo")
}

/// `<file hash>.nar.<compression>` -> `(<file hash>, <compression>)`
fn nar_file(path: &str) -> Option<(&str, &str)> {
  path.split_once(".nar.")
}

async fn get_data(
  db: &Connection,
  nix_cache: &NixCache,
  path: NarInfoPath,
  auth: Option<CliAuth>,
) -> Result<NarInfoData> {
  let Some(hash) = narinfo_hash(&path.path) else {
    bail!(NOT_FOUND, "Invalid narinfo path");
  };

  let cache = readable_cache(db, nix_cache, path.name, auth).await?;

  let Some(data) = db.nar().nar_info_data(cache.id, hash).await? else {
    bail!(NOT_FOUND, "Narinfo not found");
  };

  Ok(data)
}

async fn head_nar_info(
  db: Connection,
  nix_cache: NixCache,
  Path(path): Path<NarInfoPath>,
  auth: Option<CliAuth>,
) -> Result<HeaderMap> {
  get_data(&db, &nix_cache, path, auth).await?;

  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, NAR_INFO_MIME.clone());

  Ok(headers)
}

async fn nar_info(
  db: Connection,
  nix_cache: NixCache,
  Path(path): Path<NarInfoPath>,
  auth: Option<CliAuth>,
) -> Result<(HeaderMap, String)> {
  let data = get_data(&db, &nix_cache, path, auth).await?;
  let references = db.nar().nar_info_references(data.id).await?;

  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, NAR_INFO_MIME.clone());

  Ok((headers, format_narinfo(&data, references)?))
}

/// Same field order as nix's `NarInfo::to_string`, references and deriver as base paths
fn format_narinfo(data: &NarInfoData, mut references: Vec<String>) -> Result<String> {
  let compression = match data.compression.as_str() {
    "zst" => "zstd",
    _ => {
      bail!(NOT_FOUND, "Unsupported compression format");
    }
  };
  references.sort_unstable();

  Ok(format!(
    "StorePath: /nix/store/{}
URL: nar/{}.nar.{}
Compression: {}
FileHash: sha256:{}
FileSize: {}
NarHash: sha256:{}
NarSize: {}
References: {}{}
Sig: {}
",
    data.store_path,
    data.hash,
    data.compression,
    compression,
    data.hash,
    data.size,
    data.nar_hash,
    data.nar_size,
    references.join(" "),
    data
      .deriver
      .as_ref()
      .map(|d| format!("\nDeriver: {}", d))
      .unwrap_or_default(),
    data.signature
  ))
}

#[derive(Deserialize, JsonSchema)]
struct NarPath {
  name: String,
  hash: String,
}

/// Resolves the nar file, returns its id and size
async fn find_nar(
  db: &Connection,
  nix_cache: &NixCache,
  path: NarPath,
  auth: Option<CliAuth>,
) -> Result<(Uuid, Uuid, u64)> {
  let Some((hash, compression)) = nar_file(&path.hash) else {
    bail!(NOT_FOUND, "Invalid nar path");
  };

  let cache = readable_cache(db, nix_cache, path.name, auth).await?;

  let Some((nar_id, file_size)) = db.nar().get_nar(cache.id, hash, compression).await? else {
    bail!(NOT_FOUND, "Nar not found");
  };

  Ok((cache.id, nar_id, file_size as u64))
}

fn nar_name(nar_id: Uuid) -> object_store::path::Path {
  format!("{}.nar", nar_id).into()
}

fn nar_headers(content_length: u64) -> HeaderMap {
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, NAR_MIME.clone());
  headers.insert(header::ACCEPT_RANGES, ACCEPT_RANGES.clone());
  headers.insert(header::CONTENT_LENGTH, content_length.into());
  headers
}

/// Metadata only: no body and no access tracking
async fn head_nar(
  db: Connection,
  nix_cache: NixCache,
  Path(path): Path<NarPath>,
  storage: FileStorage,
  auth: Option<CliAuth>,
) -> Result<HeaderMap> {
  let (_, nar_id, file_size) = find_nar(&db, &nix_cache, path, auth).await?;
  if !storage.exists(&nar_name(nar_id)).await? {
    bail!(NOT_FOUND, "File not found");
  }
  Ok(nar_headers(file_size))
}

async fn nar(
  db: Connection,
  nix_cache: NixCache,
  Path(path): Path<NarPath>,
  storage: FileStorage,
  auth: Option<CliAuth>,
  headers: HeaderMap,
) -> Result<(StatusCode, HeaderMap, Body)> {
  let (cache_id, nar_id, file_size) = find_nar(&db, &nix_cache, path, auth).await?;

  let range = match headers.get(header::RANGE).and_then(|h| h.to_str().ok()) {
    Some(range) => parse_range(range, file_size),
    None => Range::Full,
  };

  let (status, range) = match range {
    Range::Full => (StatusCode::OK, None),
    Range::Partial(start, end) => (StatusCode::PARTIAL_CONTENT, Some((start, end))),
    Range::Unsatisfiable => {
      let mut headers = HeaderMap::new();
      headers.insert(header::ACCEPT_RANGES, ACCEPT_RANGES.clone());
      headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{file_size}")).unwrap(),
      );
      return Ok((StatusCode::RANGE_NOT_SATISFIABLE, headers, Body::empty()));
    }
  };

  let body = storage.get_file(&nar_name(nar_id), range).await?;
  // Only count downloads that are actually served
  tracing::info!("Serving nar {} for cache {}", nar_id, cache_id);
  db.nar().nar_accessed(nar_id, cache_id).await?;

  let headers = match range {
    Some((start, end)) => {
      let mut headers = nar_headers(end - start + 1);
      headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end}/{file_size}")).unwrap(),
      );
      headers
    }
    None => nar_headers(file_size),
  };

  Ok((status, headers, body))
}

#[derive(Debug, PartialEq, Eq)]
enum Range {
  /// No usable range: missing, malformed, other unit or multiple ranges, serve the whole file with 200
  Full,
  /// Inclusive byte range, 206
  Partial(u64, u64),
  /// 416 with `Content-Range: bytes */<size>`
  Unsatisfiable,
}

/// Single byte range per RFC 9110 14.1.2. Servers may ignore any Range header, so everything except one valid
/// `first-[last]` or `-suffix` spec is answered with the full file. Numbers too large for u64 saturate.
fn parse_range(header: &str, size: u64) -> Range {
  let Some((unit, spec)) = header.trim().split_once('=') else {
    return Range::Full;
  };
  if !unit.trim().eq_ignore_ascii_case("bytes") || spec.contains(',') {
    return Range::Full;
  }
  let Some((first, last)) = spec.trim().split_once('-') else {
    return Range::Full;
  };
  let number = |s: &str| {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().unwrap_or(u64::MAX))
  };

  let (first, last) = (first.trim(), last.trim());

  match number(first) {
    // suffix: the last N bytes
    None if first.is_empty() => match number(last) {
      Some(0) => Range::Unsatisfiable,
      Some(_) if size == 0 => Range::Unsatisfiable,
      Some(n) => Range::Partial(size - n.min(size), size - 1),
      None => Range::Full,
    },
    None => Range::Full,
    Some(first) => match if last.is_empty() {
      Some(u64::MAX)
    } else {
      number(last)
    } {
      Some(last) if last < first => Range::Full,
      Some(last) if first < size => Range::Partial(first, last.min(size - 1)),
      Some(_) => Range::Unsatisfiable,
      None => Range::Full,
    },
  }
}

#[cfg(test)]
mod test {
  use super::*;

  // ---- parse_range ----

  #[test]
  fn parse_range_table() {
    use Range::*;
    let cases: &[(&str, u64, Range)] = &[
      // first-last
      ("bytes=0-0", 10, Partial(0, 0)),
      ("bytes=0-9", 10, Partial(0, 9)),
      ("bytes=2-5", 10, Partial(2, 5)),
      ("bytes=9-9", 10, Partial(9, 9)),
      ("bytes=5-5", 10, Partial(5, 5)),
      // last past the end is clamped
      ("bytes=0-10", 10, Partial(0, 9)),
      ("bytes=3-1000", 10, Partial(3, 9)),
      // open ended
      ("bytes=0-", 10, Partial(0, 9)),
      ("bytes=9-", 10, Partial(9, 9)),
      // unsatisfiable first
      ("bytes=10-", 10, Unsatisfiable),
      ("bytes=10-20", 10, Unsatisfiable),
      ("bytes=11-11", 10, Unsatisfiable),
      // suffix
      ("bytes=-1", 10, Partial(9, 9)),
      ("bytes=-3", 10, Partial(7, 9)),
      ("bytes=-10", 10, Partial(0, 9)),
      ("bytes=-11", 10, Partial(0, 9)),
      ("bytes=-0", 10, Unsatisfiable),
      // one byte file
      ("bytes=0-0", 1, Partial(0, 0)),
      ("bytes=0-", 1, Partial(0, 0)),
      ("bytes=-1", 1, Partial(0, 0)),
      ("bytes=-5", 1, Partial(0, 0)),
      ("bytes=1-", 1, Unsatisfiable),
      // zero byte file: nothing is satisfiable, must not underflow
      ("bytes=0-", 0, Unsatisfiable),
      ("bytes=0-0", 0, Unsatisfiable),
      ("bytes=0-100", 0, Unsatisfiable),
      ("bytes=-1", 0, Unsatisfiable),
      ("bytes=-0", 0, Unsatisfiable),
      // last < first is an invalid spec, ignored
      ("bytes=5-2", 10, Full),
      ("bytes=5-2", 0, Full),
      ("bytes=1-0", 1, Full),
      // malformed
      ("", 10, Full),
      ("bytes", 10, Full),
      ("bytes=", 10, Full),
      ("bytes=-", 10, Full),
      ("bytes=abc", 10, Full),
      ("bytes=a-b", 10, Full),
      ("bytes=1-b", 10, Full),
      ("bytes=a-5", 10, Full),
      ("bytes=--5", 10, Full),
      ("bytes=+1-5", 10, Full),
      ("bytes=1-+5", 10, Full),
      ("bytes=0x1-5", 10, Full),
      ("bytes=1.5-5", 10, Full),
      ("bytes=1-2-3", 10, Full),
      ("bytes=-1-2", 10, Full),
      ("0-5", 10, Full),
      ("items=0-5", 10, Full),
      ("bites=0-5", 10, Full),
      // multiple ranges are ignored (RFC 9110 allows ignoring Range)
      ("bytes=0-1,3-4", 10, Full),
      ("bytes=0-1, 3-4", 10, Full),
      ("bytes=0-1,", 10, Full),
      ("bytes=,0-1", 10, Full),
      // unit is case insensitive, surrounding whitespace tolerated
      ("BYTES=1-2", 10, Partial(1, 2)),
      ("Bytes=1-2", 10, Partial(1, 2)),
      (" bytes = 1 - 2 ", 10, Partial(1, 2)),
      ("bytes=\t-2", 10, Partial(8, 9)),
      // numbers overflowing u64 saturate instead of panicking
      ("bytes=0-99999999999999999999999", 10, Partial(0, 9)),
      ("bytes=99999999999999999999999-", 10, Unsatisfiable),
      (
        "bytes=99999999999999999999999-99999999999999999999999",
        10,
        Unsatisfiable,
      ),
      ("bytes=-99999999999999999999999", 10, Partial(0, 9)),
      ("bytes=18446744073709551615-", u64::MAX, Unsatisfiable),
      (
        "bytes=0-18446744073709551615",
        u64::MAX,
        Partial(0, u64::MAX - 1),
      ),
      (
        "bytes=-18446744073709551615",
        u64::MAX,
        Partial(0, u64::MAX - 1),
      ),
    ];

    for (header, size, expected) in cases {
      assert_eq!(
        &parse_range(header, *size),
        expected,
        "parse_range({header:?}, {size})"
      );
    }
  }

  // ---- path helpers ----

  #[test]
  fn narinfo_hash_strips_suffix_only() {
    assert_eq!(narinfo_hash("abc.narinfo"), Some("abc"));
    assert_eq!(narinfo_hash(".narinfo"), Some(""));
    assert_eq!(narinfo_hash("abc.narinfo.narinfo"), Some("abc.narinfo"));
    assert_eq!(narinfo_hash("abc"), None);
    assert_eq!(narinfo_hash("abc.NARINFO"), None);
    assert_eq!(narinfo_hash("abc.narinfox"), None);
    assert_eq!(narinfo_hash("abc.nar"), None);
  }

  #[test]
  fn nar_file_splits_at_first_nar() {
    assert_eq!(nar_file("x.nar.zst"), Some(("x", "zst")));
    assert_eq!(nar_file("x.nar.xz"), Some(("x", "xz")));
    assert_eq!(nar_file("x.nar.zst.nar.y"), Some(("x", "zst.nar.y")));
    assert_eq!(nar_file(".nar.zst"), Some(("", "zst")));
    assert_eq!(nar_file("x.nar."), Some(("x", "")));
    assert_eq!(nar_file("x.nar"), None);
    assert_eq!(nar_file("x"), None);
    assert_eq!(nar_file(""), None);
  }

  // ---- format_narinfo ----

  fn data(compression: &str, deriver: Option<&str>) -> NarInfoData {
    NarInfoData {
      id: Uuid::nil(),
      store_path: "aaaa-hello".into(),
      compression: compression.into(),
      hash: "filehash".into(),
      size: 12,
      nar_hash: "narhash".into(),
      nar_size: 34,
      deriver: deriver.map(Into::into),
      signature: "key:c2ln".into(),
    }
  }

  #[test]
  fn format_narinfo_full() {
    let out = format_narinfo(
      &data("zst", Some("dddd-hello.drv")),
      vec!["cccc-c".into(), "aaaa-hello".into(), "bbbb-b".into()],
    )
    .unwrap();
    assert_eq!(
      out,
      "StorePath: /nix/store/aaaa-hello
URL: nar/filehash.nar.zst
Compression: zstd
FileHash: sha256:filehash
FileSize: 12
NarHash: sha256:narhash
NarSize: 34
References: aaaa-hello bbbb-b cccc-c
Deriver: dddd-hello.drv
Sig: key:c2ln
"
    );
  }

  #[test]
  fn format_narinfo_without_deriver_or_references() {
    let out = format_narinfo(&data("zst", None), vec![]).unwrap();
    assert_eq!(
      out,
      "StorePath: /nix/store/aaaa-hello
URL: nar/filehash.nar.zst
Compression: zstd
FileHash: sha256:filehash
FileSize: 12
NarHash: sha256:narhash
NarSize: 34
References: 
Sig: key:c2ln
"
    );
    assert!(!out.contains("Deriver"));
    assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
  }

  #[test]
  fn format_narinfo_sorts_many_references() {
    let refs: Vec<String> = (0..50).rev().map(|i| format!("{i:02}-ref")).collect();
    let out = format_narinfo(&data("zst", None), refs).unwrap();
    let expected: Vec<String> = (0..50).map(|i| format!("{i:02}-ref")).collect();
    assert!(out.contains(&format!("\nReferences: {}\n", expected.join(" "))));
  }

  #[test]
  fn format_narinfo_unsupported_compression_is_not_found() {
    for compression in ["xz", "none", "", "zstd", "ZST"] {
      let err = format_narinfo(&data(compression, None), vec![]).unwrap_err();
      assert_eq!(err.status, StatusCode::NOT_FOUND, "{compression}");
    }
  }
}
