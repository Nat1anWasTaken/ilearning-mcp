use std::borrow::Cow;

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

/// An RFC 6749 §5.2 error (`{"error", "error_description"}`), also used by
/// the registration (RFC 7591) and revocation (RFC 7009) endpoints.
#[derive(Debug, thiserror::Error)]
#[error("{code}: {description}")]
pub struct OAuthError {
    status: StatusCode,
    code: &'static str,
    description: Cow<'static, str>,
}

impl OAuthError {
    fn new(
        status: StatusCode,
        code: &'static str,
        description: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            status,
            code,
            description: description.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn invalid_request(description: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", description)
    }

    pub fn invalid_client() -> Self {
        Self::invalid_client_with("client authentication failed")
    }

    pub fn invalid_client_with(description: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_client", description)
    }

    pub fn invalid_grant(description: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_grant", description)
    }

    pub fn invalid_target() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "resource must be this server's MCP endpoint",
        )
    }

    pub fn unsupported_response_type() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported_response_type",
            "response_type must be code",
        )
    }

    pub fn unsupported_grant_type() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "supported grant types are authorization_code and refresh_token",
        )
    }

    pub fn invalid_redirect_uri(description: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_redirect_uri", description)
    }

    pub fn invalid_client_metadata(description: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            description,
        )
    }
}

impl From<sqlx::Error> for OAuthError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = ?err, "database error in OAuth endpoint");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "internal server error",
        )
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        (
            self.status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "error": self.code, "error_description": self.description })),
        )
            .into_response()
    }
}
