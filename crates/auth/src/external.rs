//! Verification of RS256 access tokens issued by an external OIDC provider.
//!
//! Keycloak is the default local provider. The verifier fetches the realm JWKS,
//! caches RSA public keys, validates RS256 signatures, and checks issuer,
//! audience, expiry, and scope. Keys are refreshed by the maintenance task.

use crate::config::ExternalOAuthConfig;
use crate::error::AuthError;
use crate::jwt::Claims;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use parking_lot::RwLock;
use rsa::{BigUint, Pkcs1v15Sign, RsaPublicKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
struct VerificationKey {
    kid: Option<String>,
    key: RsaPublicKey,
}

pub struct ExternalVerifier {
    config: ExternalOAuthConfig,
    client: reqwest::Client,
    keys: RwLock<Vec<VerificationKey>>,
    jwks_json: RwLock<Value>,
}

impl ExternalVerifier {
    pub async fn new(config: ExternalOAuthConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| format!("cannot build OIDC HTTP client: {error}"))?;
        let verifier = Self {
            config,
            client,
            keys: RwLock::new(Vec::new()),
            jwks_json: RwLock::new(Value::Null),
        };
        verifier.refresh().await?;
        Ok(verifier)
    }

    pub async fn refresh(&self) -> Result<(), String> {
        let response = self
            .client
            .get(&self.config.jwks_uri)
            .send()
            .await
            .map_err(|error| format!("cannot fetch JWKS from {}: {error}", self.config.jwks_uri))?;
        if !response.status().is_success() {
            return Err(format!(
                "JWKS endpoint {} returned {}",
                self.config.jwks_uri,
                response.status()
            ));
        }
        let jwks: Value = response
            .json()
            .await
            .map_err(|error| format!("invalid JWKS JSON: {error}"))?;
        let mut keys = Vec::new();
        if let Some(items) = jwks.get("keys").and_then(Value::as_array) {
            for item in items {
                if item.get("kty").and_then(Value::as_str) != Some("RSA") {
                    continue;
                }
                let (Some(n), Some(e)) = (
                    item.get("n").and_then(Value::as_str),
                    item.get("e").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let (Ok(n), Ok(e)) = (URL_SAFE_NO_PAD.decode(n), URL_SAFE_NO_PAD.decode(e)) else {
                    continue;
                };
                let Ok(key) =
                    RsaPublicKey::new(BigUint::from_bytes_be(&n), BigUint::from_bytes_be(&e))
                else {
                    continue;
                };
                keys.push(VerificationKey {
                    kid: item.get("kid").and_then(Value::as_str).map(str::to_string),
                    key,
                });
            }
        }
        if keys.is_empty() {
            return Err("JWKS contained no usable RSA signing keys".into());
        }
        *self.keys.write() = keys;
        *self.jwks_json.write() = jwks;
        Ok(())
    }

    pub fn jwks_json(&self) -> Value {
        self.jwks_json.read().clone()
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

        let header: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(header_part)
                .map_err(|_| AuthError::InvalidToken("invalid JWT header".into()))?,
        )
        .map_err(|_| AuthError::InvalidToken("invalid JWT header".into()))?;
        if header.get("alg").and_then(Value::as_str) != Some("RS256") {
            return Err(AuthError::InvalidToken(
                "external tokens must use RS256".into(),
            ));
        }
        let kid = header.get("kid").and_then(Value::as_str);
        let signature = URL_SAFE_NO_PAD
            .decode(signature_part)
            .map_err(|_| AuthError::InvalidToken("invalid JWT signature".into()))?;
        let signing_input = format!("{header_part}.{payload_part}");
        let digest = Sha256::digest(signing_input.as_bytes());
        let keys = self.keys.read();
        let mut verified = false;
        for key in keys.iter() {
            if kid.is_some() && key.kid.as_deref() != kid {
                continue;
            }
            if key
                .key
                .verify(Pkcs1v15Sign::new::<Sha256>(), &digest, &signature)
                .is_ok()
            {
                verified = true;
                break;
            }
        }
        if !verified {
            return Err(AuthError::InvalidToken(
                "external JWT signature verification failed".into(),
            ));
        }

        let payload: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload_part)
                .map_err(|_| AuthError::InvalidToken("invalid JWT payload".into()))?,
        )
        .map_err(|_| AuthError::InvalidToken("invalid JWT claims".into()))?;

        let issuer = payload
            .get("iss")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if issuer.trim_end_matches('/') != self.config.issuer.trim_end_matches('/') {
            return Err(AuthError::InvalidToken(format!(
                "unexpected token issuer '{issuer}'"
            )));
        }
        let now = chrono::Utc::now().timestamp();
        let exp = payload
            .get("exp")
            .and_then(Value::as_i64)
            .ok_or_else(|| AuthError::InvalidToken("token has no exp claim".into()))?;
        if exp + 30 < now {
            return Err(AuthError::InvalidToken("token has expired".into()));
        }
        let iat = payload.get("iat").and_then(Value::as_i64).unwrap_or(now);
        if iat > now + 60 {
            return Err(AuthError::InvalidToken("token issued in the future".into()));
        }
        if !audience_matches(payload.get("aud"), expected_audience) {
            return Err(AuthError::InvalidToken(format!(
                "token audience does not include '{expected_audience}'"
            )));
        }

        let subject = payload
            .get("sub")
            .and_then(Value::as_str)
            .ok_or_else(|| AuthError::InvalidToken("token has no sub claim".into()))?;
        let client_id = payload
            .get("azp")
            .and_then(Value::as_str)
            .or_else(|| payload.get("client_id").and_then(Value::as_str))
            .unwrap_or("external-client");
        let scope = payload
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let jti = payload
            .get("jti")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("ext-{}", &signature_part[..signature_part.len().min(24)]));
        let username = payload
            .get("preferred_username")
            .and_then(Value::as_str)
            .map(str::to_string);

        Ok(Claims {
            iss: self.config.issuer.clone(),
            sub: subject.to_string(),
            aud: expected_audience.to_string(),
            client_id: client_id.to_string(),
            scope: scope.to_string(),
            exp,
            iat,
            jti,
            resource: Some(expected_audience.to_string()),
            username,
        })
    }
}

fn audience_matches(audience: Option<&Value>, expected: &str) -> bool {
    match audience {
        Some(Value::String(value)) => value.trim_end_matches('/') == expected.trim_end_matches('/'),
        Some(Value::Array(values)) => values.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.trim_end_matches('/') == expected.trim_end_matches('/'))
        }),
        _ => false,
    }
}

pub type SharedExternalVerifier = Arc<ExternalVerifier>;

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;

    fn verifier_with_key() -> (ExternalVerifier, RsaPrivateKey) {
        let mut rng = rand::rngs::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let config = ExternalOAuthConfig {
            issuer: "http://localhost:8081/realms/docforge".into(),
            authorization_endpoint: "http://localhost:8081/auth".into(),
            token_endpoint: "http://localhost:8081/token".into(),
            jwks_uri: "http://localhost:8081/certs".into(),
            registration_endpoint: None,
            revocation_endpoint: None,
            end_session_endpoint: None,
            api_audience: "http://localhost:8080/api".into(),
            mcp_audience: "http://localhost:8080/mcp".into(),
        };
        let verifier = ExternalVerifier {
            config,
            client: reqwest::Client::new(),
            keys: RwLock::new(vec![VerificationKey {
                kid: Some("test-key".into()),
                key: public,
            }]),
            jwks_json: RwLock::new(Value::Null),
        };
        (verifier, private)
    }

    fn token(private: &RsaPrivateKey, audience: &str) -> String {
        let header = serde_json::json!({"alg":"RS256","typ":"JWT","kid":"test-key"});
        let now = chrono::Utc::now().timestamp();
        let payload = serde_json::json!({
            "iss": "http://localhost:8081/realms/docforge",
            "sub": "user-1",
            "aud": audience,
            "azp": "docforge-rest",
            "scope": "convert",
            "exp": now + 600,
            "iat": now,
            "jti": "token-1",
            "preferred_username": "demo"
        });
        let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let signing_input = format!("{header}.{payload}");
        let digest = Sha256::digest(signing_input.as_bytes());
        let signature = private
            .sign(Pkcs1v15Sign::new::<Sha256>(), &digest)
            .unwrap();
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    #[test]
    fn verifies_rs256_keycloak_tokens() {
        let (verifier, private) = verifier_with_key();
        let encoded = token(&private, "http://localhost:8080/api");
        let claims = verifier
            .verify(&encoded, "http://localhost:8080/api")
            .unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.client_id, "docforge-rest");
        assert_eq!(claims.username.as_deref(), Some("demo"));
        assert!(verifier
            .verify(&encoded, "http://localhost:8080/mcp")
            .is_err());
        let parts: Vec<&str> = encoded.split('.').collect();
        let mut tampered = parts[1].to_string();
        tampered.push('A');
        let tampered = format!("{}.{}.{}", parts[0], tampered, parts[2]);
        assert!(verifier
            .verify(&tampered, "http://localhost:8080/api")
            .is_err());
    }

    #[test]
    fn audience_arrays_are_supported() {
        let value = serde_json::json!(["account", "http://localhost:8080/mcp"]);
        assert!(audience_matches(Some(&value), "http://localhost:8080/mcp"));
        assert!(!audience_matches(Some(&value), "http://localhost:8080/api"));
    }

    #[test]
    fn audience_string_is_supported() {
        let value = serde_json::json!("http://localhost:8080/api");
        assert!(audience_matches(Some(&value), "http://localhost:8080/api/"));
    }
}
