//! PKCE S256 helpers.

use crate::error::AuthError;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub fn validate_verifier(verifier: &str) -> Result<(), AuthError> {
    if !(43..=128).contains(&verifier.len()) {
        return Err(AuthError::InvalidGrant(
            "code_verifier must be between 43 and 128 characters".into(),
        ));
    }
    if !verifier
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    {
        return Err(AuthError::InvalidGrant(
            "code_verifier contains invalid characters".into(),
        ));
    }
    Ok(())
}

pub fn challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn verify_s256(verifier: &str, challenge: &str) -> Result<(), AuthError> {
    validate_verifier(verifier)?;
    let computed = challenge_s256(verifier);
    let computed_bytes = computed.as_bytes();
    let challenge_bytes = challenge.as_bytes();
    if computed_bytes.len() != challenge_bytes.len()
        || computed_bytes.ct_eq(challenge_bytes).unwrap_u8() != 1
    {
        return Err(AuthError::InvalidGrant(
            "PKCE code_verifier does not match code_challenge".into(),
        ));
    }
    Ok(())
}

pub fn validate_challenge(challenge: &str) -> Result<(), AuthError> {
    if !(43..=128).contains(&challenge.len()) {
        return Err(AuthError::InvalidRequest(
            "code_challenge must be between 43 and 128 characters".into(),
        ));
    }
    if !challenge
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    {
        return Err(AuthError::InvalidRequest(
            "code_challenge contains invalid characters".into(),
        ));
    }
    Ok(())
}

pub fn random_urlsafe(bytes: usize) -> String {
    use rand::RngCore;
    let mut buffer = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(challenge_s256(verifier), expected);
        verify_s256(verifier, expected).unwrap();
        assert!(verify_s256(verifier, "wrong").is_err());
    }

    #[test]
    fn rejects_short_verifier() {
        assert!(validate_verifier("short").is_err());
    }

    #[test]
    fn random_tokens_are_unique() {
        assert_ne!(random_urlsafe(32), random_urlsafe(32));
        assert_eq!(Sha256::digest(b"x").len(), 32);
    }
}
