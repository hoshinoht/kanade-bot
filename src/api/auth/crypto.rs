//! Random ids, hashing and constant-time checks on `ring` (already the
//! crate's crypto provider).

use ring::{
    digest::{SHA256, digest},
    hmac,
    rand::{SecureRandom, SystemRandom},
};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url.
pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub fn decode_base64url(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for chunk in text.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, byte) in chunk.iter().enumerate() {
            let value = ALPHABET.iter().position(|c| c == byte)? as u32;
            n |= value << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    // Refuse non-canonical trailing bits so each token has exactly one spelling.
    (base64url(&out) == text).then_some(out)
}

/// 32 random bytes as 43 base64url characters.
pub fn random_token() -> Option<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(base64url(&bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn sha256_base64url(bytes: &[u8]) -> String {
    base64url(digest(&SHA256, bytes).as_ref())
}

/// A secret kept only as an HMAC tag under a random per-process key, so it
/// can be checked in constant time without holding it in plain text.
pub struct SealedSecret {
    key: hmac::Key,
    tag: hmac::Tag,
}

impl SealedSecret {
    pub fn new(secret: &[u8]) -> Option<Self> {
        let key = hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new()).ok()?;
        let tag = hmac::sign(&key, secret);
        Some(Self { key, tag })
    }

    pub fn matches(&self, candidate: &[u8]) -> bool {
        // ring recomputes the tag and compares in constant time, whatever the candidate's length.
        hmac::verify(&self.key, candidate, self.tag.as_ref()).is_ok()
    }
}

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealedSecret(..)")
    }
}

/// Constant-time equality for equal-length inputs; unequal lengths are unequal.
pub fn constant_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// HMAC-SHA256 keyed by `key` over `message`, base64url.
pub fn keyed_tag(key: &[u8], message: &[u8]) -> String {
    base64url(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), message).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips_and_matches_rfc4648_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(plain.as_bytes()), encoded);
            assert_eq!(decode_base64url(encoded).unwrap(), plain.as_bytes());
        }
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(decode_base64url("Zh"), None, "non-canonical tail");
        assert_eq!(decode_base64url("Z"), None);
        assert_eq!(decode_base64url("Zm9v!"), None);
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_appendix_b() {
        assert_eq!(
            sha256_base64url(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn sealed_secrets_match_only_the_exact_bytes() {
        let sealed = SealedSecret::new(b"correct horse battery staple").unwrap();
        assert!(sealed.matches(b"correct horse battery staple"));
        assert!(!sealed.matches(b"correct horse battery stapl"));
        assert!(!sealed.matches(b""));
        assert_eq!(format!("{sealed:?}"), "SealedSecret(..)");
        let token = random_token().unwrap();
        assert_eq!(token.len(), 43);
        assert_eq!(decode_base64url(&token).unwrap().len(), 32);
    }
}
