//! Token revocation (RFC 7009).

use axum::{
    Form,
    extract::{State, rejection::FormRejection},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;

use super::{OAuthError, client_auth};
use crate::{crypto::hash_token, state::AppState};

/// `token_type_hint` is accepted but unused: both token tables are checked.
#[derive(Deserialize)]
pub struct RevokeRequest {
    token: Option<String>,
    #[serde(flatten)]
    credentials: client_auth::FormCredentials,
}

/// Revoking a refresh token revokes its whole family, access tokens included. Unknown tokens, and
/// tokens of other clients, are ignored with a `200` as the RFC requires.
pub async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Result<Form<RevokeRequest>, FormRejection>,
) -> Result<StatusCode, OAuthError> {
    let Form(request) = form.map_err(|err| OAuthError::invalid_request(err.body_text()))?;
    let client = client_auth::authenticate(&state, &headers, &request.credentials).await?;
    let token = request
        .token
        .ok_or_else(|| OAuthError::invalid_request("token is missing"))?;

    // The token is either an access token, or a refresh token whose family goes.
    sqlx::query(
        "WITH family AS (
             SELECT r.family_id FROM oauth_refresh_tokens r
             JOIN agent_sessions s ON s.id = r.agent_session_id
             WHERE r.token_hash = $1 AND s.client_id = $2
         ), access AS (
             DELETE FROM oauth_access_tokens t USING agent_sessions s
             WHERE s.id = t.agent_session_id AND s.client_id = $2
               AND (t.token_hash = $1 OR t.family_id IN (SELECT family_id FROM family))
         )
         DELETE FROM oauth_refresh_tokens WHERE family_id IN (SELECT family_id FROM family)",
    )
    .bind(hash_token(&token))
    .bind(&client.client_id)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::OK)
}
