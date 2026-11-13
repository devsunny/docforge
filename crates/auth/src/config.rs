//! Authorization-server configuration.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// DocForge's built-in authorization server.
    #[default]
    Embedded,
    /// An external OpenID Connect provider such as Keycloak.
    Keycloak,
}

#[derive(Debug, Clone)]
pub struct ExternalOAuthConfig {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    pub end_session_endpoint: Option<String>,
    /// JWT audiences placed by Keycloak audience mappers.
    pub api_audience: String,
    pub mcp_audience: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resource {
    Api,
    Mcp,
}

impl Resource {
    pub fn scope(self) -> &'static str {
        match self {
            Resource::Api => "convert",
            Resource::Mcp => "mcp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Resource::Api => "DocForge REST API",
            Resource::Mcp => "DocForge MCP server",
        }
    }

    pub fn well_known_path(self) -> &'static str {
        match self {
            Resource::Api => "/.well-known/oauth-protected-resource/api",
            Resource::Mcp => "/.well-known/oauth-protected-resource/mcp",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Issuer of the authorization server used by clients. In Keycloak mode
    /// this is the realm URL.
    pub issuer: String,
    /// Base URL where DocForge serves its own protected-resource metadata.
    pub public_base_url: String,
    pub mode: AuthMode,
    pub external: Option<ExternalOAuthConfig>,
    pub rest_resource: String,
    pub mcp_resource: String,
    pub username: String,
    pub password: String,
    pub demo_mode: bool,
    pub access_token_ttl: Duration,
    pub refresh_token_ttl: Duration,
    pub authorization_code_ttl: Duration,
    pub dcr_enabled: bool,
    pub dcr_initial_access_token: Option<String>,
    pub max_clients: usize,
    pub allowed_origins: Vec<String>,
    pub documentation_url: String,
}

impl AuthConfig {
    pub fn for_test(issuer: &str) -> Self {
        Self {
            issuer: issuer.trim_end_matches('/').to_string(),
            public_base_url: issuer.trim_end_matches('/').to_string(),
            mode: AuthMode::Embedded,
            external: None,
            rest_resource: format!("{}/api", issuer.trim_end_matches('/')),
            mcp_resource: format!("{}/mcp", issuer.trim_end_matches('/')),
            username: "demo".to_string(),
            password: "demo".to_string(),
            demo_mode: true,
            access_token_ttl: Duration::from_secs(3600),
            refresh_token_ttl: Duration::from_secs(86_400),
            authorization_code_ttl: Duration::from_secs(300),
            dcr_enabled: true,
            dcr_initial_access_token: None,
            max_clients: 10_000,
            allowed_origins: vec![
                issuer.trim_end_matches('/').to_string(),
                "http://localhost:3000".to_string(),
                "http://127.0.0.1:3000".to_string(),
            ],
            documentation_url: format!("{}/docs", issuer.trim_end_matches('/')),
        }
    }

    pub fn resource_url(&self, resource: Resource) -> &str {
        match resource {
            Resource::Api => &self.rest_resource,
            Resource::Mcp => &self.mcp_resource,
        }
    }

    pub fn resource_from_uri(&self, resource: &str) -> Option<Resource> {
        let resource = resource.trim_end_matches('/');
        if resource == self.rest_resource.trim_end_matches('/') {
            Some(Resource::Api)
        } else if resource == self.mcp_resource.trim_end_matches('/') {
            Some(Resource::Mcp)
        } else {
            None
        }
    }

    pub fn authorization_server_issuer(&self) -> &str {
        self.external
            .as_ref()
            .map(|external| external.issuer.as_str())
            .unwrap_or(&self.issuer)
    }

    pub fn protected_resource_metadata(&self, resource: Resource) -> serde_json::Value {
        serde_json::json!({
            "resource": self.resource_url(resource),
            "authorization_servers": [self.authorization_server_issuer()],
            "scopes_supported": scopes_for(resource),
            "bearer_methods_supported": ["header"],
            "resource_name": resource.name(),
            "resource_documentation": match resource {
                Resource::Api => format!("{}/docs/api", self.public_base_url),
                Resource::Mcp => format!("{}/docs/mcp", self.public_base_url),
            },
        })
    }

    pub fn authorization_server_metadata(&self) -> serde_json::Value {
        if let Some(external) = &self.external {
            return serde_json::json!({
                "issuer": external.issuer,
                "authorization_endpoint": external.authorization_endpoint,
                "token_endpoint": external.token_endpoint,
                "registration_endpoint": external.registration_endpoint,
                "jwks_uri": external.jwks_uri,
                "revocation_endpoint": external.revocation_endpoint,
                "end_session_endpoint": external.end_session_endpoint,
                "scopes_supported": supported_scopes(),
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials"],
                "token_endpoint_auth_methods_supported": ["none", "client_secret_basic", "client_secret_post"],
                "code_challenge_methods_supported": ["S256"],
                "authorization_response_iss_parameter_supported": true,
                "service_documentation": self.documentation_url,
                "resource_documentation": format!("{}/docs/oauth", self.public_base_url),
                "ui_locales_supported": ["en"],
            });
        }
        serde_json::json!({
            "issuer": self.issuer,
            "authorization_endpoint": format!("{}/oauth/authorize", self.issuer),
            "token_endpoint": format!("{}/oauth/token", self.issuer),
            "registration_endpoint": if self.dcr_enabled { serde_json::Value::String(format!("{}/oauth/register", self.issuer)) } else { serde_json::Value::Null },
            "jwks_uri": format!("{}/oauth/jwks", self.issuer),
            "revocation_endpoint": format!("{}/oauth/revoke", self.issuer),
            "scopes_supported": supported_scopes(),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials"],
            "token_endpoint_auth_methods_supported": ["none", "client_secret_basic", "client_secret_post"],
            "code_challenge_methods_supported": ["S256"],
            "authorization_response_iss_parameter_supported": true,
            "service_documentation": self.documentation_url,
            "resource_documentation": format!("{}/docs/oauth", self.issuer),
            "ui_locales_supported": ["en"],
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_absolute_url(&self.issuer, "issuer")?;
        validate_absolute_url(&self.public_base_url, "public_base_url")?;
        validate_absolute_url(&self.rest_resource, "rest_resource")?;
        validate_absolute_url(&self.mcp_resource, "mcp_resource")?;
        if self.issuer.ends_with('/') {
            return Err("issuer must not end with '/'".into());
        }
        if self.mode == AuthMode::Embedded {
            if self.username.trim().is_empty() {
                return Err("AUTH_USERNAME must not be empty".into());
            }
            if self.password.is_empty() {
                return Err("AUTH_PASSWORD must not be empty".into());
            }
        } else {
            let external = self.external.as_ref().ok_or_else(|| {
                "AUTH_MODE=keycloak requires external OAuth endpoints".to_string()
            })?;
            for (name, value) in [
                ("external issuer", &external.issuer),
                ("authorization endpoint", &external.authorization_endpoint),
                ("token endpoint", &external.token_endpoint),
                ("jwks_uri", &external.jwks_uri),
                ("api audience", &external.api_audience),
                ("mcp audience", &external.mcp_audience),
            ] {
                validate_absolute_url(value, name)?;
            }
            if let Some(registration) = &external.registration_endpoint {
                validate_absolute_url(registration, "registration endpoint")?;
            }
        }
        if self.max_clients == 0 {
            return Err("max_clients must be > 0".into());
        }
        Ok(())
    }

    pub fn resource_metadata_url(&self, resource: Resource) -> String {
        format!("{}{}", self.public_base_url, resource.well_known_path())
    }

    pub fn is_external(&self) -> bool {
        self.external.is_some()
    }
}

fn validate_absolute_url(value: &str, name: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("{name} is not a valid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{name} must use http or https"));
    }
    Ok(())
}

pub fn supported_scopes() -> Vec<&'static str> {
    vec!["convert", "convert:read", "mcp", "offline_access"]
}

pub fn scopes_for(resource: Resource) -> Vec<&'static str> {
    match resource {
        Resource::Api => vec!["convert", "convert:read"],
        Resource::Mcp => vec!["mcp"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_contains_required_fields() {
        let config = AuthConfig::for_test("http://127.0.0.1:8080");
        config.validate().unwrap();
        let metadata = config.authorization_server_metadata();
        assert_eq!(metadata["issuer"], "http://127.0.0.1:8080");
        assert_eq!(metadata["code_challenge_methods_supported"][0], "S256");
        assert!(metadata["registration_endpoint"]
            .as_str()
            .unwrap()
            .ends_with("/oauth/register"));
        let protected = config.protected_resource_metadata(Resource::Mcp);
        assert_eq!(protected["resource"], "http://127.0.0.1:8080/mcp");
    }
}
