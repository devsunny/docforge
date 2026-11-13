//! OAuth 2.1 authorization server, PKCE, dynamic client registration, and
//! RFC 9728 protected-resource metadata for DocForge.

pub mod config;
pub mod error;
pub mod external;
pub mod jwt;
pub mod middleware;
pub mod pkce;
pub mod routes;
pub mod store;

pub use config::{AuthConfig, AuthMode, ExternalOAuthConfig, Resource};
pub use error::AuthError;
pub use jwt::{AuthContext, Claims, SigningKeys};
pub use store::AuthState;

use std::sync::Arc;
use std::time::Duration;

pub fn oauth_router(state: Arc<AuthState>) -> axum::Router {
    routes::router(state)
}

/// Periodically remove expired codes, refresh tokens, and revocation markers.
pub fn spawn_cleanup(state: Arc<AuthState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            state.store.cleanup();
            if let Some(external) = &state.external {
                if let Err(error) = external.refresh().await {
                    tracing::warn!(%error, "could not refresh external OIDC JWKS");
                }
            }
        }
    })
}
