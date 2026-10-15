//! OAuth 2.1 authorization server for MCP clients, and the bearer middleware
//! protecting the MCP endpoint.

mod authorize;
mod bearer;
mod cimd;
mod cleanup;
mod client;
mod client_assertion;
mod client_auth;
pub mod consent;
mod error;
mod metadata;
mod pkce;
mod redirect_uri;
mod register;
mod revoke;
#[cfg(test)]
mod tests;
mod token;

use axum::{
    Router,
    http::Method,
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};

use crate::{config::Config, state::AppState};

pub use bearer::{AgentPrincipal, require_agent};
pub use cleanup::spawn as spawn_cleanup;
pub use client_assertion::ClientAssertions;
use error::OAuthError;

/// The only scope; every token grants access to all of the user's iLearning tools.
pub const SCOPE: &str = "ilearning";

/// RFC 8707: a `resource` parameter, when sent, must name the MCP endpoint.
fn check_resource(config: &Config, resource: Option<&str>) -> Result<(), OAuthError> {
    match resource {
        Some(resource) if !config.is_mcp_resource(resource) => Err(OAuthError::invalid_target()),
        _ => Ok(()),
    }
}

pub fn router() -> Router<AppState> {
    // Endpoints browser-based clients (e.g. MCP Inspector) call with `fetch`.
    let cross_origin = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(metadata::protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(metadata::protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(metadata::authorization_server),
        )
        .route(
            "/.well-known/openid-configuration",
            get(metadata::authorization_server),
        )
        .route("/oauth/register", post(register::register))
        .route("/oauth/token", post(token::token))
        .route("/oauth/revoke", post(revoke::revoke))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers(Any),
        );
    Router::new()
        .route("/oauth/authorize", get(authorize::authorize))
        .merge(cross_origin)
}
