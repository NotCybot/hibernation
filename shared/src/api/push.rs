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

#[cfg(test)]
mod test {
  use serde_json::json;
  use uuid::Uuid;

  use super::*;

  #[test]
  fn wire_format_round_trips() {
    let id = Uuid::new_v4();
    let req = UploadPathRequest {
      cache: id,
      force: true,
      store_path: "abc-hello".into(),
      nar_hash: "hash".into(),
      nar_size: 7,
      deriver: None,
      references: vec!["abc-dep".into()],
      signature: "k:sig".into(),
    };
    let value = serde_json::to_value(&req).unwrap();
    assert_eq!(
      value,
      json!({
        "cache": id, "force": true, "store_path": "abc-hello", "nar_hash": "hash", "nar_size": 7,
        "deriver": null, "references": ["abc-dep"], "signature": "k:sig",
      })
    );
    let back: UploadPathRequest = serde_json::from_value(value).unwrap();
    assert_eq!(back.references, req.references);
    assert_eq!(back.deriver, None);

    // Missing optional deriver is accepted, missing required fields are not
    let mut partial = serde_json::to_value(&req).unwrap();
    partial.as_object_mut().unwrap().remove("deriver");
    assert!(serde_json::from_value::<UploadPathRequest>(partial.clone()).is_ok());
    partial.as_object_mut().unwrap().remove("nar_size");
    assert!(serde_json::from_value::<UploadPathRequest>(partial).is_err());

    let info: UploadInfoRequest =
      serde_json::from_value(json!({ "cache": "main", "paths": ["a"], "force": false })).unwrap();
    assert_eq!(info.paths, ["a"]);

    let res: UploadInfoResponse =
      serde_json::from_value(json!({ "paths": ["a"], "cache": id })).unwrap();
    assert_eq!(res.cache, id);

    let res: UploadPathResponse =
      serde_json::from_value(json!({ "uuid": id, "nar_found": true })).unwrap();
    assert!(res.nar_found);

    let finish = UploadFinishRequest {
      file_hash: "h".into(),
      file_size: 3,
    };
    assert_eq!(
      serde_json::to_value(&finish).unwrap(),
      json!({ "file_hash": "h", "file_size": 3 })
    );
  }
}
