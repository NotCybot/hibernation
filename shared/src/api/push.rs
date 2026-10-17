use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Size of a single NAR upload chunk
pub const CHUNK_SIZE: usize = 32 * 1024 * 1024;

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UploadInfoRequest {
  pub cache: String,
  pub paths: Vec<String>,
  pub force: bool,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema)]
pub struct UploadInfoResponse {
  pub paths: Vec<String>,
  pub cache: Uuid,
}

#[derive(Deserialize, Serialize, Debug, JsonSchema)]
pub struct UploadPathRequest {
  pub cache: Uuid,
  pub force: bool,
  pub store_path: String,
  pub nar_hash: String,
  pub nar_size: u64,
  pub deriver: Option<String>,
  pub references: Vec<String>,
  pub signature: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct UploadPathResponse {
  pub uuid: Uuid,
  /// The NAR already exists on the server, no chunks need to be uploaded
  pub nar_found: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct UploadFinishRequest {
  pub file_hash: String,
  pub file_size: u64,
}
