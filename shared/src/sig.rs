use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_compact::Signature;
use harmonia_store_core::store_path::{StoreDir, StorePath};

#[derive(Clone)]
pub struct KeyPair {
  name: String,
  keypair: ed25519_compact::KeyPair,
}

#[derive(Clone)]
pub struct PublicKey {
  name: String,
  public_key: ed25519_compact::PublicKey,
}

impl KeyPair {
  pub fn from_string(s: &str) -> Option<Self> {
    let parts = s.splitn(2, ':').collect::<Vec<_>>();
    // nix rejects keys without a name as corrupt
    if parts.len() != 2 || parts[0].is_empty() {
      return None;
    }

    let name = parts[0].to_string();
    let keypair_bytes = BASE64_STANDARD.decode(parts[1]).ok()?;
    let keypair = ed25519_compact::KeyPair::from_slice(&keypair_bytes).ok()?;

    Some(Self { name, keypair })
  }

  pub fn export_public_key(&self) -> String {
    format!("{}:{}", self.name, BASE64_STANDARD.encode(*self.keypair.pk))
  }

  pub fn export_keypair(&self) -> String {
    format!("{}:{}", self.name, BASE64_STANDARD.encode(*self.keypair))
  }

  pub fn to_public_key(&self) -> PublicKey {
    PublicKey {
      name: self.name.clone(),
      public_key: self.keypair.pk,
    }
  }

  pub fn sign(
    &self,
    path: &StorePath,
    nar_hash: &str,
    nar_size: u64,
    references: &[StorePath],
  ) -> String {
    let message = fingerprint(path, nar_hash, nar_size, references);
    let sig = BASE64_STANDARD.encode(self.keypair.sk.sign(message, None));
    format!("{}:{}", self.name, sig)
  }

  pub fn verify(
    &self,
    signature: &str,
    path: &StorePath,
    nar_hash: &str,
    nar_size: u64,
    references: &[StorePath],
  ) -> bool {
    self
      .to_public_key()
      .verify(signature, path, nar_hash, nar_size, references)
  }
}

impl PublicKey {
  pub fn from_string(s: &str) -> Option<Self> {
    let parts = s.splitn(2, ':').collect::<Vec<_>>();
    // nix rejects keys without a name as corrupt
    if parts.len() != 2 || parts[0].is_empty() {
      return None;
    }

    let name = parts[0].to_string();
    let public_key_bytes = BASE64_STANDARD.decode(parts[1]).ok()?;
    let public_key = ed25519_compact::PublicKey::from_slice(&public_key_bytes).ok()?;

    Some(Self { name, public_key })
  }

  pub fn verify(
    &self,
    signature: &str,
    path: &StorePath,
    nar_hash: &str,
    nar_size: u64,
    references: &[StorePath],
  ) -> bool {
    let message = fingerprint(path, nar_hash, nar_size, references);

    let parts = signature.splitn(2, ':').collect::<Vec<_>>();
    if parts.len() != 2 {
      return false;
    }

    let sig_name = parts[0];
    let sig_value = parts[1];

    if sig_name != self.name {
      return false;
    }

    let Some(signature) = BASE64_STANDARD
      .decode(sig_value)
      .ok()
      .and_then(|bytes| Signature::from_slice(&bytes).ok())
    else {
      return false;
    };

    self.public_key.verify(message, &signature).is_ok()
  }
}

fn fingerprint(
  path: &StorePath,
  nar_hash: &str,
  nar_size: u64,
  references: &[StorePath],
) -> Vec<u8> {
  let store_dir = StoreDir::default();

  // 1;{storePath};{narHash};{narSize};{commaDelimitedReferences}
  let mut fingerprint = b"1;".to_vec();

  let path_str = store_dir.display(path).to_string();
  fingerprint.extend_from_slice(path_str.as_bytes());
  fingerprint.push(b';');

  let nar_hash = format!("sha256:{}", nar_hash);
  fingerprint.extend_from_slice(nar_hash.as_bytes());
  fingerprint.push(b';');

  let mut buf = itoa::Buffer::new();
  let nar_size_str = buf.format(nar_size);
  fingerprint.extend_from_slice(nar_size_str.as_bytes());
  fingerprint.push(b';');

  let mut iter = references.iter().peekable();
  while let Some(ref_path) = iter.next() {
    let ref_str = store_dir.display(ref_path).to_string();
    fingerprint.extend_from_slice(ref_str.as_bytes());

    if iter.peek().is_some() {
      fingerprint.push(b',');
    }
  }

  fingerprint
}

#[cfg(test)]
mod test {
  use base64::{Engine, prelude::BASE64_STANDARD};
  use harmonia_store_core::store_path::{StoreDir, StorePath};

  use super::{KeyPair, PublicKey, fingerprint};

  const SECRET: &str = "testing:iDTfQV9yVOwU/VFPVW6Zh/l9Ibdh1W6KurGI6KAVF/hKRlL9bp2evH0Bj/+LceiF4gbzl7sSgS8Fcj8fTiIeJA==";
  const PUBLIC: &str = "testing:SkZS/W6dnrx9AY//i3HoheIG85e7EoEvBXI/H04iHiQ=";
  const HASH: &str = "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73";

  fn path(s: &str) -> StorePath {
    StoreDir::default()
      .parse(&format!("/nix/store/{s}"))
      .unwrap()
  }

  fn a() -> StorePath {
    path("7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-a")
  }

  fn b() -> StorePath {
    path("x5sv0d8nf0spww4qyjn3zqd3k7w00s4m-b")
  }

  fn main_path() -> StorePath {
    path("1b8m03r63zqhnjf7l5wnldhh7c134ap5-hello-1.0")
  }

  #[test]
  fn keypair_parses_and_exports() {
    let key = KeyPair::from_string(SECRET).unwrap();
    assert_eq!(key.export_keypair(), SECRET);
    assert_eq!(key.export_public_key(), PUBLIC);
    assert!(PublicKey::from_string(PUBLIC).is_some());
  }

  #[test]
  fn from_string_rejects_malformed() {
    let pk = PUBLIC.split_once(':').unwrap().1;
    let sk = SECRET.split_once(':').unwrap().1;
    for bad in [
      String::new(),
      "no-colon".into(),
      ":".into(),
      format!(":{sk}"),
      "testing:not base64!".into(),
      format!("testing:{}", BASE64_STANDARD.encode([1u8; 63])),
      format!("testing:{pk}"),
    ] {
      assert!(KeyPair::from_string(&bad).is_none(), "keypair {bad:?}");
    }
    for bad in [
      String::new(),
      "no-colon".into(),
      format!(":{pk}"),
      "testing:not base64!".into(),
      format!("testing:{}", BASE64_STANDARD.encode([1u8; 31])),
      format!("testing:{sk}"),
    ] {
      assert!(PublicKey::from_string(&bad).is_none(), "public key {bad:?}");
    }
  }

  #[test]
  fn colon_in_value_is_part_of_base64() {
    // splitn(2): only the first colon separates the name
    assert!(KeyPair::from_string(&format!("{SECRET}:extra")).is_none());
  }

  #[test]
  fn fingerprint_matches_nix_format() {
    // https://github.com/NixOS/nix/blob/master/src/libstore/path-info.cc `ValidPathInfo::fingerprint`
    let fp = fingerprint(&main_path(), HASH, 1234, &[a(), b()]);
    assert_eq!(
      String::from_utf8(fp).unwrap(),
      format!(
        "1;/nix/store/1b8m03r63zqhnjf7l5wnldhh7c134ap5-hello-1.0;sha256:{HASH};1234;\
/nix/store/7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-a,/nix/store/x5sv0d8nf0spww4qyjn3zqd3k7w00s4m-b"
      )
    );
    let fp = fingerprint(&main_path(), HASH, 0, &[]);
    assert_eq!(
      String::from_utf8(fp).unwrap(),
      format!("1;/nix/store/1b8m03r63zqhnjf7l5wnldhh7c134ap5-hello-1.0;sha256:{HASH};0;")
    );
  }

  #[test]
  fn sign_verify_round_trip() {
    let key = KeyPair::from_string(SECRET).unwrap();
    let public = PublicKey::from_string(PUBLIC).unwrap();
    let refs = [a(), b()];
    let sig = key.sign(&main_path(), HASH, 42, &refs);
    assert!(sig.starts_with("testing:"));
    // ed25519 signatures are 64 bytes
    let raw = BASE64_STANDARD
      .decode(sig.split_once(':').unwrap().1)
      .unwrap();
    assert_eq!(raw.len(), 64);
    // deterministic
    assert_eq!(sig, key.sign(&main_path(), HASH, 42, &refs));

    assert!(key.verify(&sig, &main_path(), HASH, 42, &refs));
    assert!(public.verify(&sig, &main_path(), HASH, 42, &refs));
    assert!(
      key
        .to_public_key()
        .verify(&sig, &main_path(), HASH, 42, &refs)
    );
  }

  #[test]
  fn verify_rejects_tampering() {
    let key = KeyPair::from_string(SECRET).unwrap();
    let public = key.to_public_key();
    let refs = [a(), b()];
    let sig = key.sign(&main_path(), HASH, 42, &refs);

    assert!(!public.verify(&sig, &a(), HASH, 42, &refs), "path");
    assert!(
      !public.verify(&sig, &main_path(), &HASH.replace('0', "1"), 42, &refs),
      "hash"
    );
    assert!(!public.verify(&sig, &main_path(), HASH, 43, &refs), "size");
    assert!(!public.verify(&sig, &main_path(), HASH, 42, &[a()]), "refs");
    assert!(
      !public.verify(&sig, &main_path(), HASH, 42, &[b(), a()]),
      "ref order"
    );

    // Signature format
    let value = sig.split_once(':').unwrap().1;
    for bad in [
      value.to_string(),
      format!("other:{value}"),
      "testing:not base64!".to_string(),
      format!("testing:{}", BASE64_STANDARD.encode([0u8; 63])),
      format!("testing:{}", BASE64_STANDARD.encode([0u8; 64])),
    ] {
      assert!(!public.verify(&bad, &main_path(), HASH, 42, &refs), "{bad}");
    }
  }

  #[test]
  fn verify_rejects_other_key_same_name() {
    let other = ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([7; 32]));
    let other =
      KeyPair::from_string(&format!("testing:{}", BASE64_STANDARD.encode(*other))).unwrap();
    let sig = other.sign(&main_path(), HASH, 42, &[]);
    let public = PublicKey::from_string(PUBLIC).unwrap();
    assert!(!public.verify(&sig, &main_path(), HASH, 42, &[]));
    assert!(other.verify(&sig, &main_path(), HASH, 42, &[]));
  }
}
