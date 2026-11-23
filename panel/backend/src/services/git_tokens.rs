use rand::RngCore;
use sha2::{Digest, Sha256};

/// Token prefix for built-in git server personal access tokens.
/// Distinguished from `arsy_` API keys so logs and extractors can identify the family.
pub const GIT_TOKEN_PREFIX: &str = "arsygit_";

/// Generate a new `arsygit_` token: `arsygit_` + 40 lowercase hex characters (160 bits of entropy).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 20];
    rand::rng().fill_bytes(&mut bytes);
    format!("{GIT_TOKEN_PREFIX}{}", hex::encode(bytes))
}

/// Compute the SHA-256 hex digest stored in `git_tokens.token_hash`.
/// Mirrors `routes/api_keys.rs` hashing discipline.
pub fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// Extract the display prefix kept in `git_tokens.token_prefix` (first 12 chars).
pub fn display_prefix(raw: &str) -> String {
    raw.chars().take(12).collect()
}

/// Constant-time comparison of two hex SHA-256 hashes.
pub fn ct_hash_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_hash_roundtrip() {
        let raw = generate_token();
        assert!(raw.starts_with(GIT_TOKEN_PREFIX));
        assert_eq!(raw.len(), GIT_TOKEN_PREFIX.len() + 40);
        assert!(raw[GIT_TOKEN_PREFIX.len()..]
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f')));

        let h1 = hash_token(&raw);
        let h2 = hash_token(&raw);
        assert_eq!(h1.len(), 64);
        assert!(ct_hash_eq(&h1, &h2));
        assert!(!ct_hash_eq(&h1, &hash_token(&generate_token())));
    }
}
