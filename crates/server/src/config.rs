//! Environment-driven server configuration.

use docforge_auth::{AuthConfig, AuthMode, ExternalOAuthConfig};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind: String,
    pub public_base_url: String,
    pub data_dir: PathBuf,
    pub max_upload_bytes: usize,
    pub max_sync_bytes: usize,
    pub conversion_concurrency: usize,
    pub conversion_timeout_secs: u64,
    pub job_ttl: Duration,
    pub cors_allow_any: bool,
    pub cors_origins: Vec<String>,
    pub mcp_max_inline_bytes: usize,
    pub auth: AuthConfig,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let public_base_url = env_or("PUBLIC_BASE_URL", "http://127.0.0.1:8080")
            .trim_end_matches('/')
            .to_string();
        let bind = env_or("BIND_ADDR", "127.0.0.1:8080");
        let mode = match env_or("AUTH_MODE", "embedded")
            .to_ascii_lowercase()
            .as_str()
        {
            "keycloak" | "external" => AuthMode::Keycloak,
            "embedded" | "local" => AuthMode::Embedded,
            other => {
                return Err(format!(
                    "AUTH_MODE must be embedded or keycloak, got '{other}'"
                ))
            }
        };
        let mut issuer = env_or("AUTH_ISSUER", &public_base_url)
            .trim_end_matches('/')
            .to_string();
        let rest_resource = env_or("AUTH_REST_RESOURCE", &format!("{public_base_url}/api"));
        let mcp_resource = env_or("AUTH_MCP_RESOURCE", &format!("{public_base_url}/mcp"));
        let external = if mode == AuthMode::Keycloak {
            let keycloak_issuer =
                env_or("KEYCLOAK_ISSUER", "http://localhost:8081/realms/docforge")
                    .trim_end_matches('/')
                    .to_string();
            issuer = keycloak_issuer.clone();
            Some(ExternalOAuthConfig {
                issuer: keycloak_issuer.clone(),
                authorization_endpoint: env_or(
                    "KEYCLOAK_AUTHORIZATION_ENDPOINT",
                    &format!("{keycloak_issuer}/protocol/openid-connect/auth"),
                ),
                token_endpoint: env_or(
                    "KEYCLOAK_TOKEN_ENDPOINT",
                    &format!("{keycloak_issuer}/protocol/openid-connect/token"),
                ),
                jwks_uri: env_or(
                    "KEYCLOAK_JWKS_URL",
                    &format!("{keycloak_issuer}/protocol/openid-connect/certs"),
                ),
                registration_endpoint: Some(env_or(
                    "KEYCLOAK_REGISTRATION_ENDPOINT",
                    &format!("{keycloak_issuer}/clients-registrations/openid-connect"),
                )),
                revocation_endpoint: Some(env_or(
                    "KEYCLOAK_REVOCATION_ENDPOINT",
                    &format!("{keycloak_issuer}/protocol/openid-connect/revoke"),
                )),
                end_session_endpoint: Some(env_or(
                    "KEYCLOAK_END_SESSION_ENDPOINT",
                    &format!("{keycloak_issuer}/protocol/openid-connect/logout"),
                )),
                api_audience: env_or("AUTH_API_AUDIENCE", &rest_resource),
                mcp_audience: env_or("AUTH_MCP_AUDIENCE", &mcp_resource),
            })
        } else {
            None
        };
        let allowed_origins = split_csv(&env_or(
            "AUTH_ALLOWED_ORIGINS",
            &format!(
                "{public_base_url},http://localhost:3000,http://127.0.0.1:3000,http://localhost:6274,http://127.0.0.1:6274"
            ),
        ));
        let auth = AuthConfig {
            issuer,
            public_base_url: public_base_url.clone(),
            mode,
            external,
            rest_resource,
            mcp_resource,
            username: env_or("AUTH_USERNAME", "demo"),
            password: env_or("AUTH_PASSWORD", "demo"),
            demo_mode: env_bool("AUTH_DEMO_MODE", true),
            access_token_ttl: Duration::from_secs(env_u64("AUTH_ACCESS_TOKEN_TTL", 3600)),
            refresh_token_ttl: Duration::from_secs(env_u64(
                "AUTH_REFRESH_TOKEN_TTL",
                60 * 60 * 24 * 30,
            )),
            authorization_code_ttl: Duration::from_secs(env_u64(
                "AUTH_AUTHORIZATION_CODE_TTL",
                300,
            )),
            dcr_enabled: env_bool("AUTH_DCR_ENABLED", true),
            dcr_initial_access_token: std::env::var("AUTH_DCR_INITIAL_ACCESS_TOKEN")
                .ok()
                .filter(|value| !value.is_empty()),
            max_clients: env_u64("AUTH_MAX_CLIENTS", 10_000) as usize,
            allowed_origins: allowed_origins.clone(),
            documentation_url: format!("{public_base_url}/docs"),
        };
        auth.validate()?;

        let data_dir = PathBuf::from(env_or("DATA_DIR", "./data"));
        let cors_allow_any = env_bool("CORS_ALLOW_ANY", false);
        Ok(Self {
            bind,
            public_base_url,
            data_dir,
            max_upload_bytes: env_u64("MAX_UPLOAD_MB", 100) as usize * 1024 * 1024,
            max_sync_bytes: env_u64("MAX_SYNC_MB", 25) as usize * 1024 * 1024,
            conversion_concurrency: env_u64("CONVERSION_CONCURRENCY", 4).max(1) as usize,
            conversion_timeout_secs: env_u64("CONVERSION_TIMEOUT_SECS", 300).max(1),
            job_ttl: Duration::from_secs(env_u64("JOB_TTL_HOURS", 24).max(1) * 3600),
            cors_allow_any,
            cors_origins: split_csv(&env_or("CORS_ALLOWED_ORIGINS", &allowed_origins.join(","))),
            mcp_max_inline_bytes: env_u64("MCP_MAX_INLINE_MB", 16) as usize * 1024 * 1024,
            auth,
        })
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.jobs_dir())?;
        std::fs::create_dir_all(self.temp_dir())?;
        Ok(())
    }

    pub fn download_url(&self, job_id: &str) -> String {
        format!("{}/api/v1/jobs/{job_id}/download", self.public_base_url)
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        // Environment-dependent but the defaults must parse.
        let config = AppConfig::from_env().unwrap();
        assert!(config.max_upload_bytes > 0);
        assert!(config.conversion_concurrency >= 1);
        assert!(config.auth.issuer.starts_with("http"));
    }

    #[test]
    fn split_csv_trims() {
        assert_eq!(split_csv("a, b,,c"), vec!["a", "b", "c"]);
    }
}
