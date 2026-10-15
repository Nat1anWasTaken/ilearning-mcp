//! The dashboard half of authorization: the consent page reads a pending
//! request and posts the user's decision, which yields the client redirect.

use anyhow::Context;
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::redirect_uri;
use crate::{
    auth::CurrentUser,
    crypto::{hash_token, random_token},
    error::{AppError, AppResult},
    sessions,
    state::AppState,
};

const CODE_TTL: TimeDelta = TimeDelta::minutes(5);

#[derive(Serialize, FromRow)]
pub struct PendingRequest {
    client_id: String,
    client_name: Option<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
    #[sqlx(skip)]
    origin: Option<String>,
    redirect_uri: String,
    scope: String,
    /// The user's active session for this client, when re-authorizing.
    #[sqlx(json(nullable))]
    existing_session: Option<ExistingSession>,
}

#[derive(Serialize, Deserialize)]
struct ExistingSession {
    name: String,
}

pub async fn show(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<PendingRequest>> {
    let mut request: PendingRequest = sqlx::query_as(
        "SELECT c.client_id, c.client_name, c.client_uri, c.logo_uri, r.redirect_uri, r.scope,
                to_jsonb(s) AS existing_session
         FROM oauth_requests r
         JOIN oauth_clients c ON c.client_id = r.client_id
         LEFT JOIN LATERAL (
             SELECT name FROM agent_sessions
             WHERE user_id = $2 AND client_id = r.client_id AND revoked_at IS NULL
         ) s ON true
         WHERE r.id = $1 AND r.expires_at > now()",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    request.origin = redirect_uri::origin(&request.redirect_uri);
    Ok(Json(request))
}

/// The user's answer. On approval `name` overrides the default name of a new
/// session, or renames the session being re-authorized.
#[derive(Deserialize)]
pub struct Decision {
    approve: bool,
    name: Option<String>,
}

#[derive(FromRow)]
struct ConsentedRequest {
    client_id: String,
    client_name: Option<String>,
    redirect_uri: String,
    state: Option<String>,
    scope: String,
    resource: Option<String>,
    code_challenge: String,
}

pub async fn decide(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(decision): Json<Decision>,
) -> AppResult<Json<Value>> {
    let name = decision
        .name
        .as_deref()
        .map(sessions::normalize_name)
        .transpose()?;

    let mut tx = state.db.begin().await?;
    let request: ConsentedRequest = sqlx::query_as(
        "DELETE FROM oauth_requests r USING oauth_clients c
         WHERE r.id = $1 AND r.expires_at > now() AND c.client_id = r.client_id
         RETURNING r.client_id, c.client_name, r.redirect_uri, r.state, r.scope, r.resource,
                   r.code_challenge",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;

    let outcome = if decision.approve {
        let session_id = upsert_session(&mut tx, user.id, &request, name).await?;
        ("code", issue_code(&mut tx, session_id, &request).await?)
    } else {
        ("error", "access_denied".to_owned())
    };
    let params = [outcome, ("iss", state.config.public_url.clone())]
        .into_iter()
        .chain(request.state.map(|s| ("state", s)));
    let redirect_to = redirect_uri::with_params(&request.redirect_uri, params)
        .context("building authorization response")?;
    tx.commit().await?;
    Ok(Json(json!({ "redirect_to": redirect_to })))
}

/// Re-authorizing a client reuses the user's active session for it.
async fn upsert_session(
    tx: &mut PgConnection,
    user_id: Uuid,
    request: &ConsentedRequest,
    name: Option<String>,
) -> sqlx::Result<Uuid> {
    let origin = redirect_uri::origin(&request.redirect_uri);
    let default_name = request
        .client_name
        .clone()
        .or_else(|| origin.clone())
        .unwrap_or_else(|| request.client_id.clone());
    sqlx::query_scalar(
        "INSERT INTO agent_sessions (user_id, client_id, name, origin)
         VALUES ($1, $2, COALESCE($3, $4), $5)
         ON CONFLICT (user_id, client_id) WHERE revoked_at IS NULL DO UPDATE SET
             name = COALESCE($3, agent_sessions.name),
             origin = EXCLUDED.origin
         RETURNING id",
    )
    .bind(user_id)
    .bind(&request.client_id)
    .bind(name)
    .bind(default_name)
    .bind(origin)
    .fetch_one(tx)
    .await
}

async fn issue_code(
    tx: &mut PgConnection,
    session_id: Uuid,
    request: &ConsentedRequest,
) -> sqlx::Result<String> {
    let code = random_token();
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, agent_session_id, redirect_uri,
             code_challenge, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(hash_token(&code))
    .bind(&request.client_id)
    .bind(session_id)
    .bind(&request.redirect_uri)
    .bind(&request.code_challenge)
    .bind(&request.scope)
    .bind(&request.resource)
    .bind(Utc::now() + CODE_TTL)
    .execute(tx)
    .await?;
    Ok(code)
}
