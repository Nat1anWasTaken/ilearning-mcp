//! Bearer token authentication for the MCP endpoint (RFC 6750).

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{config::username_allowed_sql, crypto::hash_token, state::AppState};

/// The agent session a request was authenticated as, stored in the request
/// extensions by [`require_agent`]. The session is never revoked.
#[derive(Debug, Clone, Copy, FromRow)]
pub struct AgentPrincipal {
    pub user_id: Uuid,
    pub session_id: Uuid,
}

/// Middleware (for `middleware::from_fn_with_state`) that rejects requests
/// without a valid access token with `401` and a `WWW-Authenticate` challenge
/// pointing at the protected resource metadata.
pub async fn require_agent(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(request.headers()) else {
        return challenge(&state, None);
    };
    match authenticate(&state.db, token, &state.config.allowed_moodle_usernames).await {
        Ok(Some(principal)) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Ok(None) => challenge(&state, Some("invalid_token")),
        Err(err) => {
            tracing::error!(error = ?err, "authenticating bearer token");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let (scheme, token) = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

/// Resolves a live access token of a non-revoked session whose owner is still
/// allowed to sign in, bumping the session's `last_used_at` at most once a minute.
async fn authenticate(
    db: &PgPool,
    token: &str,
    allowed_usernames: &[String],
) -> sqlx::Result<Option<AgentPrincipal>> {
    sqlx::query_as(concat!(
        "WITH hit AS (
             SELECT s.id, s.user_id, s.last_used_at
             FROM oauth_access_tokens t
             JOIN agent_sessions s ON s.id = t.agent_session_id
             JOIN users u ON u.id = s.user_id
             WHERE t.token_hash = $1 AND t.expires_at > now() AND s.revoked_at IS NULL
               AND ",
        username_allowed_sql!(),
        "
         ), bump AS (
             UPDATE agent_sessions SET last_used_at = now()
             WHERE id IN (SELECT id FROM hit
                          WHERE last_used_at IS NULL OR last_used_at < now() - interval '1 minute')
         )
         SELECT user_id, id AS session_id FROM hit",
    ))
    .bind(hash_token(token))
    .bind(allowed_usernames)
    .fetch_optional(db)
    .await
}

fn challenge(state: &AppState, error: Option<&str>) -> Response {
    let mut value = format!(
        r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource/mcp""#,
        state.config.public_url
    );
    if let Some(error) = error {
        value.push_str(&format!(r#", error="{error}""#));
    }
    match HeaderValue::from_str(&value) {
        Ok(value) => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, value)],
        )
            .into_response(),
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
}
