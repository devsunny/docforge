//! Compact Ed25519 JWT issuance and verification with a JWKS endpoint.

use crate::error::AuthError;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub client_id: String,
    pub scope: String,
    pub exp: i64,
    pub iat: i64,
    pub jti: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl Claims {
    pub fn scopes(&self) -> Vec<String> {
        self.scope.split_whitespace().map(str::to_string).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthContext {
    pub subject: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub audience: String,
    pub token_id: String,
    pub resource: Option<String>,
}

impl AuthContext {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|value| value == scope)
    }

    pub fn from_claims(claims: &Claims) -> Self {
        Self {
            subject: claims.sub.clone(),
            client_id: claims.client_id.clone(),
            scopes: claims.scopes(),
            audience: claims.aud.clone(),
            token_id: claims.jti.clone(),
            resource: claims.resource.clone(),
        }
    }
}

pub struct SigningKeys {
    signing: SigningKey,
    verifying: VerifyingKey,
    kid: String,
    issuer: String,
}

impl SigningKeys {
    pub fn generate(issuer: impl Into<String>) -> Self {
        let signing = SigningKey::generate(&mut OsRng);
        let verifying = signing.verifying_key();
        let mut hasher = Sha256::new();
        hasher.update(verifying.as_bytes());
        let digest = hasher.finalize();
        let kid = URL_SAFE_NO_PAD.encode(&digest[..8]);
        Self {
            signing,
            verifying,
            kid,
            issuer: issuer.into(),
        }
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn sign(&self, claims: &Claims) -> Result<String, AuthError> {
        let header = serde_json::json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": self.kid,
        });
        let header = serde_json::to_vec(&header)
            .map_err(|error| AuthError::ServerError(error.to_string()))?;
        let payload = serde_json::to_vec(claims)
            .map_err(|error| AuthError::ServerError(error.to_string()))?;
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header),
            URL_SAFE_NO_PAD.encode(payload)
        );
        let signature = self.signing.sign(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }

    pub fn verify(&self, token: &str, expected_audience: &str) -> Result<Claims, AuthError> {
        let mut parts = token.split('.');
        let header_part = parts
            .next()
            .ok_or_else(|| AuthError::InvalidToken("malformed JWT".into()))?;
        let payload_part = parts
            .next()
            .ok_or_else(|| AuthError::InvalidToken("malformed JWT".into()))?;
        let signature_part = parts
            .next()
            .ok_or_else(|| AuthError::InvalidToken("malformed JWT".into()))?;
        if parts.next().is_some() {
            return Err(AuthError::InvalidToken("malformed JWT".into()));
        }

        let header_bytes = URL_SAFE_NO_PAD
            .decode(header_part)
            .map_err(|_| AuthError::InvalidToken("invalid JWT header encoding".into()))?;
        let header_json: serde_json::Value = serde_json::from_slice(&header_bytes)
            .map_err(|_| AuthError::InvalidToken("invalid JWT header".into()))?;
        if header_json["alg"].as_str() != Some("EdDSA") {
            return Err(AuthError::InvalidToken("unsupported JWT algorithm".into()));
        }
        if header_json["kid"].as_str() != Some(self.kid.as_str()) {
            return Err(AuthError::InvalidToken("unknown signing key".into()));
        }

        let signature = URL_SAFE_NO_PAD
            .decode(signature_part)
            .map_err(|_| AuthError::InvalidToken("invalid JWT signature encoding".into()))?;
        let signature = Signature::from_slice(&signature)
            .map_err(|_| AuthError::InvalidToken("invalid Ed25519 signature".into()))?;
        let signing_input = format!("{header_part}.{payload_part}");
        self.verifying
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| AuthError::InvalidToken("JWT signature verification failed".into()))?;

        let payload = URL_SAFE_NO_PAD
            .decode(payload_part)
            .map_err(|_| AuthError::InvalidToken("invalid JWT payload encoding".into()))?;
        let claims: Claims = serde_json::from_slice(&payload)
            .map_err(|_| AuthError::InvalidToken("invalid JWT claims".into()))?;

        let now = chrono::Utc::now().timestamp();
        if claims.exp + 30 < now {
            return Err(AuthError::InvalidToken("token has expired".into()));
        }
        if claims.iat > now + 60 {
            return Err(AuthError::InvalidToken("token issued in the future".into()));
        }
        if claims.iss != self.issuer {
            return Err(AuthError::InvalidToken("unexpected token issuer".into()));
        }
        if claims.aud != expected_audience {
            return Err(AuthError::InvalidToken(format!(
                "token audience is {}, expected {}",
                claims.aud, expected_audience
            )));
        }
        Ok(claims)
    }

    pub fn jwks(&self) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "x": URL_SAFE_NO_PAD.encode(self.verifying.as_bytes()),
                "use": "sig",
                "alg": "EdDSA",
                "kid": self.kid,
            }]
        })
    }

    pub fn verify_signature_only(&self, token: &str) -> Result<Claims, AuthError> {
        let mut parts = token.split('.');
        let header = parts
            .next()
            .ok_or_else(|| AuthError::InvalidToken("malformed JWT".into()))?;
        let payload = parts
            .next()
            .ok_or_else(|| AuthError::InvalidToken("malformed JWT".into()))?;
        let signature = parts
            .next()
            .ok_or_else(|| AuthError::InvalidToken("malformed JWT".into()))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| AuthError::InvalidToken("invalid JWT signature encoding".into()))?;
        let signature = Signature::from_slice(&signature)
            .map_err(|_| AuthError::InvalidToken("invalid Ed25519 signature".into()))?;
        self.verifying
            .verify(format!("{header}.{payload}").as_bytes(), &signature)
            .map_err(|_| AuthError::InvalidToken("JWT signature verification failed".into()))?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| AuthError::InvalidToken("invalid JWT payload encoding".into()))?;
        serde_json::from_slice(&payload)
            .map_err(|_| AuthError::InvalidToken("invalid JWT claims".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(issuer: &str, audience: &str) -> Claims {
        let now = chrono::Utc::now().timestamp();
        Claims {
            iss: issuer.to_string(),
            sub: "user-1".into(),
            aud: audience.into(),
            client_id: "client-1".into(),
            scope: "convert mcp".into(),
            exp: now + 600,
            iat: now,
            jti: "jti-1".into(),
            resource: Some(audience.into()),
            username: Some("demo".into()),
        }
    }

    #[test]
    fn signs_and_verifies() {
        let keys = SigningKeys::generate("https://auth.example.com");
        let token = keys
            .sign(&claims(
                "https://auth.example.com",
                "https://api.example.com",
            ))
            .unwrap();
        let parsed = keys.verify(&token, "https://api.example.com").unwrap();
        assert_eq!(parsed.sub, "user-1");
        assert_eq!(parsed.scopes(), vec!["convert", "mcp"]);
    }

    #[test]
    fn rejects_wrong_audience_and_tampering() {
        let keys = SigningKeys::generate("https://auth.example.com");
        let token = keys
            .sign(&claims(
                "https://auth.example.com",
                "https://api.example.com",
            ))
            .unwrap();
        assert!(keys.verify(&token, "https://mcp.example.com").is_err());
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[1] = "AAAA";
        assert!(keys
            .verify(&parts.join("."), "https://api.example.com")
            .is_err());
    }

    #[test]
    fn jwks_has_ed25519_key() {
        let keys = SigningKeys::generate("https://auth.example.com");
        let jwks = keys.jwks();
        assert_eq!(jwks["keys"][0]["kty"], "OKP");
        assert_eq!(jwks["keys"][0]["crv"], "Ed25519");
    }
}
