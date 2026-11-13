//! In-memory OAuth state.
//!
//! This adapter is intentionally small and dependency-free. For a multi-node
//! deployment, replace `AuthStore` behind the same methods with a database or
//! Redis implementation; tokens are opaque random values stored as SHA-256
//! hashes, never plaintext.

use crate::config::AuthConfig;
use crate::error::AuthError;
use crate::jwt::{Claims, SigningKeys};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use parking_lot::RwLock;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

pub fn hash_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub fn constant_time_eq(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).unwrap_u8() == 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Client {
    pub client_id: String,
    #[serde(skip_serializing)]
    pub client_secret_hash: Option<String>,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub scope: String,
    pub client_id_issued_at: i64,
    #[serde(skip_serializing)]
    pub registration_token_hash: Option<String>,
    #[serde(flatten)]
    pub metadata: serde_json::Value,
}

impl Client {
    pub fn is_public(&self) -> bool {
        self.token_endpoint_auth_method == "none"
    }

    pub fn allows_grant(&self, grant: &str) -> bool {
        self.grant_types.iter().any(|value| value == grant)
    }

    pub fn public_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "client_id": self.client_id,
            "client_id_issued_at": self.client_id_issued_at,
            "client_name": self.client_name,
            "redirect_uris": self.redirect_uris,
            "grant_types": self.grant_types,
            "response_types": self.response_types,
            "token_endpoint_auth_method": self.token_endpoint_auth_method,
            "scope": self.scope,
        });
        if let serde_json::Value::Object(object) = &mut value {
            if let serde_json::Value::Object(extra) = &self.metadata {
                for (key, value) in extra {
                    object.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        value
    }
}

#[derive(Debug, Clone)]
pub struct AuthCode {
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub scope: String,
    pub resource: String,
    pub subject: String,
    pub username: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub token: String,
    pub client_id: String,
    pub subject: String,
    pub username: String,
    pub scope: String,
    pub resource: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewClient {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub scope: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct RegisteredClient {
    pub client: Client,
    pub client_secret: Option<String>,
    pub registration_access_token: Option<String>,
}

#[derive(Default)]
struct Inner {
    clients: HashMap<String, Client>,
    codes: HashMap<String, AuthCode>,
    refresh: HashMap<String, RefreshToken>,
    revoked_jti: HashMap<String, i64>,
}

pub struct AuthStore {
    inner: RwLock<Inner>,
    max_clients: usize,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStore {
    pub fn new() -> Self {
        Self::with_limit(10_000)
    }

    pub fn with_limit(max_clients: usize) -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            max_clients: max_clients.max(1),
        }
    }

    pub fn client(&self, client_id: &str) -> Option<Client> {
        self.inner.read().clients.get(client_id).cloned()
    }

    pub fn client_count(&self) -> usize {
        self.inner.read().clients.len()
    }

    pub fn register_client(&self, new: NewClient) -> Result<RegisteredClient, AuthError> {
        let mut inner = self.inner.write();
        if inner.clients.len() >= self.max_clients {
            return Err(AuthError::TemporarilyUnavailable(
                "client registration limit reached".into(),
            ));
        }
        let client_id = format!("fb_{}", random_token(18));
        let confidential = new.token_endpoint_auth_method != "none";
        let client_secret = confidential.then(|| format!("fbs_{}", random_token(32)));
        let registration_access_token = Some(format!("fbr_{}", random_token(32)));
        let client = Client {
            client_id: client_id.clone(),
            client_secret_hash: client_secret.as_deref().map(hash_token),
            client_name: new.client_name,
            redirect_uris: new.redirect_uris,
            grant_types: new.grant_types,
            response_types: new.response_types,
            token_endpoint_auth_method: new.token_endpoint_auth_method,
            scope: new.scope,
            client_id_issued_at: now(),
            registration_token_hash: registration_access_token.as_deref().map(hash_token),
            metadata: new.metadata,
        };
        inner.clients.insert(client_id, client.clone());
        Ok(RegisteredClient {
            client,
            client_secret,
            registration_access_token,
        })
    }

    pub fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
    ) -> Result<Client, AuthError> {
        let client = self
            .inner
            .read()
            .clients
            .get(client_id)
            .cloned()
            .ok_or(AuthError::InvalidClient)?;
        if client.is_public() {
            if let Some(secret) = client_secret {
                if !secret.is_empty() {
                    return Err(AuthError::InvalidClient);
                }
            }
            return Ok(client);
        }
        let Some(expected) = &client.client_secret_hash else {
            return Err(AuthError::InvalidClient);
        };
        let Some(provided) = client_secret else {
            return Err(AuthError::InvalidClient);
        };
        if !constant_time_eq(expected, &hash_token(provided)) {
            return Err(AuthError::InvalidClient);
        }
        Ok(client)
    }

    pub fn registration_token_valid(&self, client_id: &str, token: &str) -> bool {
        self.inner
            .read()
            .clients
            .get(client_id)
            .and_then(|client| client.registration_token_hash.as_deref())
            .is_some_and(|expected| constant_time_eq(expected, &hash_token(token)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_code(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code_challenge: &str,
        scope: &str,
        resource: &str,
        subject: &str,
        username: &str,
        ttl_seconds: u64,
    ) -> String {
        let code = random_token(32);
        let entry = AuthCode {
            code: code.clone(),
            client_id: client_id.to_string(),
            redirect_uri: redirect_uri.to_string(),
            code_challenge: code_challenge.to_string(),
            scope: scope.to_string(),
            resource: resource.to_string(),
            subject: subject.to_string(),
            username: username.to_string(),
            expires_at: now() + ttl_seconds as i64,
        };
        self.inner.write().codes.insert(hash_token(&code), entry);
        code
    }

    pub fn take_code(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: Option<&str>,
    ) -> Result<AuthCode, AuthError> {
        let key = hash_token(code);
        let entry = self.inner.write().codes.remove(&key);
        let entry = entry.ok_or_else(|| {
            AuthError::InvalidGrant("authorization code is invalid or already used".into())
        })?;
        if entry.expires_at < now() {
            return Err(AuthError::InvalidGrant(
                "authorization code has expired".into(),
            ));
        }
        if entry.client_id != client_id {
            return Err(AuthError::InvalidGrant(
                "authorization code was issued to another client".into(),
            ));
        }
        if entry.redirect_uri != redirect_uri {
            return Err(AuthError::InvalidGrant(
                "redirect_uri does not match the authorization request".into(),
            ));
        }
        if let Some(resource) = resource {
            if entry.resource.trim_end_matches('/') != resource.trim_end_matches('/') {
                return Err(AuthError::InvalidGrant(
                    "resource does not match the authorization request".into(),
                ));
            }
        }
        Ok(entry)
    }

    pub fn issue_refresh(
        &self,
        client_id: &str,
        subject: &str,
        username: &str,
        scope: &str,
        resource: &str,
        ttl_seconds: u64,
    ) -> String {
        let token = random_token(40);
        let entry = RefreshToken {
            token: token.clone(),
            client_id: client_id.to_string(),
            subject: subject.to_string(),
            username: username.to_string(),
            scope: scope.to_string(),
            resource: resource.to_string(),
            expires_at: now() + ttl_seconds as i64,
        };
        self.inner.write().refresh.insert(hash_token(&token), entry);
        token
    }

    pub fn rotate_refresh(
        &self,
        token: &str,
        client_id: &str,
        requested_scope: Option<&str>,
        ttl_seconds: u64,
    ) -> Result<(RefreshToken, String), AuthError> {
        let key = hash_token(token);
        let entry = self.inner.write().refresh.remove(&key).ok_or_else(|| {
            AuthError::InvalidGrant("refresh token is invalid or already rotated".into())
        })?;
        if entry.expires_at < now() {
            return Err(AuthError::InvalidGrant("refresh token has expired".into()));
        }
        if entry.client_id != client_id {
            return Err(AuthError::InvalidGrant(
                "refresh token was issued to another client".into(),
            ));
        }
        let scope = if let Some(requested) = requested_scope {
            let allowed: HashSet<&str> = entry.scope.split_whitespace().collect();
            let requested: Vec<&str> = requested.split_whitespace().collect();
            if requested.iter().any(|scope| !allowed.contains(scope)) {
                return Err(AuthError::InvalidScope(
                    "requested scope exceeds the refresh token scope".into(),
                ));
            }
            requested.join(" ")
        } else {
            entry.scope.clone()
        };
        let next = random_token(40);
        let new_entry = RefreshToken {
            token: next.clone(),
            client_id: entry.client_id.clone(),
            subject: entry.subject.clone(),
            username: entry.username.clone(),
            scope: scope.clone(),
            resource: entry.resource.clone(),
            expires_at: now() + ttl_seconds as i64,
        };
        self.inner
            .write()
            .refresh
            .insert(hash_token(&next), new_entry.clone());
        Ok((new_entry, next))
    }

    pub fn revoke(&self, token: &str, claims_jti: Option<&str>) {
        let mut inner = self.inner.write();
        inner.refresh.remove(&hash_token(token));
        if let Some(jti) = claims_jti {
            inner.revoked_jti.insert(jti.to_string(), now() + 86_400);
        }
    }

    pub fn is_revoked(&self, jti: &str) -> bool {
        self.inner.read().revoked_jti.contains_key(jti)
    }

    pub fn cleanup(&self) {
        let now = now();
        let mut inner = self.inner.write();
        inner.codes.retain(|_, code| code.expires_at >= now);
        inner.refresh.retain(|_, token| token.expires_at >= now);
        inner.revoked_jti.retain(|_, expires| *expires >= now);
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<AuthConfig>,
    pub keys: Arc<SigningKeys>,
    pub store: Arc<AuthStore>,
    pub external: Option<Arc<crate::external::ExternalVerifier>>,
    pub started_at: i64,
}

impl AuthState {
    pub fn new(config: AuthConfig) -> Self {
        let keys = Arc::new(SigningKeys::generate(config.issuer.clone()));
        let max_clients = config.max_clients;
        Self {
            config: Arc::new(config),
            keys,
            store: Arc::new(AuthStore::with_limit(max_clients)),
            external: None,
            started_at: now(),
        }
    }

    /// Build the auth state. In Keycloak mode this fetches the realm JWKS
    /// before the HTTP server starts.
    pub async fn initialize(config: AuthConfig) -> Result<Self, String> {
        let mut state = Self::new(config);
        if let Some(external_config) = state.config.external.clone() {
            let mut last_error = String::new();
            let mut verifier = None;
            // Keycloak can still be importing its realm when Compose starts
            // DocForge; retry the JWKS fetch briefly.
            for attempt in 0..30 {
                match crate::external::ExternalVerifier::new(external_config.clone()).await {
                    Ok(value) => {
                        verifier = Some(value);
                        break;
                    }
                    Err(error) => {
                        last_error = error;
                        if attempt < 29 {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                    }
                }
            }
            let verifier = verifier
                .ok_or_else(|| format!("could not initialize external OAuth JWKS: {last_error}"))?;
            state.external = Some(Arc::new(verifier));
        }
        Ok(state)
    }

    pub fn is_external(&self) -> bool {
        self.external.is_some()
    }

    pub fn verify_access(
        &self,
        token: &str,
        resource: crate::config::Resource,
    ) -> Result<Claims, AuthError> {
        if let Some(external) = &self.external {
            let audience = match resource {
                crate::config::Resource::Api => self
                    .config
                    .external
                    .as_ref()
                    .map(|config| config.api_audience.as_str())
                    .unwrap_or_else(|| self.config.resource_url(resource)),
                crate::config::Resource::Mcp => self
                    .config
                    .external
                    .as_ref()
                    .map(|config| config.mcp_audience.as_str())
                    .unwrap_or_else(|| self.config.resource_url(resource)),
            };
            external.verify(token, audience)
        } else {
            self.keys.verify(token, self.config.resource_url(resource))
        }
    }

    pub fn external_jwks(&self) -> Option<serde_json::Value> {
        self.external.as_ref().map(|external| external.jwks_json())
    }

    pub fn check_password(&self, username: &str, password: &str) -> bool {
        constant_time_eq(username, &self.config.username)
            & constant_time_eq(password, &self.config.password)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_secret_authentication() {
        let store = AuthStore::new();
        let registered = store
            .register_client(NewClient {
                client_name: "Test".into(),
                redirect_uris: vec!["http://127.0.0.1:9000/cb".into()],
                grant_types: vec!["authorization_code".into()],
                response_types: vec!["code".into()],
                token_endpoint_auth_method: "client_secret_basic".into(),
                scope: "convert".into(),
                metadata: serde_json::json!({}),
            })
            .unwrap();
        let secret = registered.client_secret.unwrap();
        assert!(store
            .authenticate_client(&registered.client.client_id, Some(&secret))
            .is_ok());
        assert!(store
            .authenticate_client(&registered.client.client_id, Some("wrong"))
            .is_err());
    }

    #[test]
    fn authorization_code_is_single_use() {
        let store = AuthStore::new();
        let code = store.create_code(
            "client",
            "http://127.0.0.1/cb",
            "challenge",
            "convert",
            "http://127.0.0.1/api",
            "user",
            "demo",
            60,
        );
        assert!(store
            .take_code(
                &code,
                "client",
                "http://127.0.0.1/cb",
                Some("http://127.0.0.1/api")
            )
            .is_ok());
        assert!(store
            .take_code(
                &code,
                "client",
                "http://127.0.0.1/cb",
                Some("http://127.0.0.1/api")
            )
            .is_err());
    }
}
