/// https://github.com/kolloch/nix-base32/blob/master/src/lib.rs
const BASE32_CHARS: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";

pub fn to_nix_base32(bytes: &[u8]) -> String {
  if bytes.is_empty() {
    return String::new();
  }
  let len = (bytes.len() * 8 - 1) / 5 + 1;

  (0..len)
    .rev()
    .map(|n| {
      let b: usize = n * 5;
      let i: usize = b / 8;
      let j: usize = b % 8;
      // bits from the lower byte
      let v1 = bytes[i].checked_shr(j as u32).unwrap_or(0);
      // bits from the upper byte
      let v2 = if i >= bytes.len() - 1 {
        0
      } else {
        bytes[i + 1].checked_shl(8 - j as u32).unwrap_or(0)
      };
      let v: usize = (v1 | v2) as usize;
      char::from(BASE32_CHARS[v % BASE32_CHARS.len()])
    })
    .collect()
}

#[cfg(test)]
mod test {
  use super::to_nix_base32;

  fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
      .step_by(2)
      .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
      .collect()
  }

  /// Expected values from `nix hash convert --hash-algo <algo> --to nix32 <hex>`
  #[test]
  fn matches_nix_hash_convert() {
    let cases = [
      // sha256("")
      (
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73",
      ),
      // sha256("abc")
      (
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s",
      ),
      // sha256("hello world")
      (
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9",
        "1sfdxziarxw8j3p80lvswgpq9i7smdyxmmsj5sjhhgjdjfwjfkdr",
      ),
      // sha1("abc")
      (
        "a9993e364706816aba3e25717850c26c9cd0d89d",
        "kpcd173cq987hw957sx6m0868wv3x6d9",
      ),
      // sha512("abc")
      (
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "2gs8k559z4rlahfx0y688s49m2vvszylcikrfinm30ly9rak69236nkam5ydvly1ai7xac99vxfc4ii84hawjbk876blyk1jfhkbbyx",
      ),
    ];
    for (input, expected) in cases {
      assert_eq!(to_nix_base32(&hex(input)), expected, "{input}");
    }
  }

  #[test]
  fn length_and_alphabet() {
    // 32 bytes -> ceil(256 / 5) = 52 chars, never e/o/u/t
    let out = to_nix_base32(&[0xff; 32]);
    assert_eq!(out.len(), 52);
    assert!(
      out
        .chars()
        .all(|c| "0123456789abcdfghijklmnpqrsvwxyz".contains(c))
    );
    assert_eq!(to_nix_base32(&[0; 32]), "0".repeat(52));
  }

  #[test]
  fn short_inputs() {
    assert_eq!(to_nix_base32(&[]), "");
    assert_eq!(to_nix_base32(&[0x1f]), "0z");
    assert_eq!(to_nix_base32(&[0xff]), "7z");
  }
}
