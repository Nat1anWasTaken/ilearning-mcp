//! The token endpoint: authorization code exchange and refresh token rotation.

use axum::{
    Form, Json,
    extract::{State, rejection::FormRejection},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgExecutor, PgPool};
use uuid::Uuid;

use super::{OAuthError, check_resource, client::Client, client_auth, pkce};
use crate::{
    crypto::{hash_token, random_token},
    state::AppState,
};

const ACCESS_TOKEN_TTL: TimeDelta = TimeDelta::hours(1);
const REFRESH_TOKEN_TTL: TimeDelta = TimeDelta::days(30);

#[derive(Deserialize)]
pub struct TokenRequest {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
    #[serde(flatten)]
    credentials: client_auth::FormCredentials,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    refresh_token: String,
    scope: String,
}

pub async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Result<Form<TokenRequest>, FormRejection>,
) -> Result<impl IntoResponse, OAuthError> {
    let Form(request) = form.map_err(|err| OAuthError::invalid_request(err.body_text()))?;
    let client = client_auth::authenticate(&state, &headers, &request.credentials).await?;
    check_resource(&state.config, request.resource.as_deref())?;
    let tokens = match request.grant_type.as_deref() {
        Some("authorization_code") => exchange_code(&state.db, &client, &request).await?,
        Some("refresh_token") => refresh(&state.db, &client, &request).await?,
        Some(_) => return Err(OAuthError::unsupported_grant_type()),
        None => return Err(OAuthError::invalid_request("grant_type is missing")),
    };
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(tokens),
    ))
}

#[derive(FromRow)]
struct CodeGrant {
    client_id: String,
    agent_session_id: Uuid,
    redirect_uri: String,
    code_challenge: String,
    scope: String,
    expired: bool,
    session_revoked: bool,
}

async fn exchange_code(
    db: &PgPool,
    client: &Client,
    request: &TokenRequest,
) -> Result<TokenResponse, OAuthError> {
    let (Some(code), Some(redirect_uri), Some(verifier)) = (
        request.code.as_deref(),
        request.redirect_uri.as_deref(),
        request.code_verifier.as_deref(),
    ) else {
        return Err(OAuthError::invalid_request(
            "code, redirect_uri and code_verifier are required",
        ));
    };
    // Deleting first makes the code single-use even if the exchange fails below.
    let grant: CodeGrant = sqlx::query_as(
        "DELETE FROM oauth_codes c USING agent_sessions s
         WHERE c.code_hash = $1 AND s.id = c.agent_session_id
         RETURNING c.client_id, c.agent_session_id, c.redirect_uri, c.code_challenge, c.scope,
                   c.expires_at <= now() AS expired, s.revoked_at IS NOT NULL AS session_revoked",
    )
    .bind(hash_token(code))
    .fetch_optional(db)
    .await?
    .ok_or_else(|| OAuthError::invalid_grant("authorization code is invalid"))?;

    if grant.client_id != client.client_id || grant.expired || grant.session_revoked {
        return Err(OAuthError::invalid_grant(
            "authorization code is invalid or expired",
        ));
    }
    if grant.redirect_uri != redirect_uri {
        return Err(OAuthError::invalid_grant(
            "redirect_uri does not match the authorization request",
        ));
    }
    if !pkce::verify(verifier, &grant.code_challenge) {
        return Err(OAuthError::invalid_grant("code_verifier does not match"));
    }

    Ok(issue(db, grant.agent_session_id, &grant.scope, Uuid::new_v4()).await?)
}

#[derive(FromRow)]
struct RefreshGrant {
    agent_session_id: Uuid,
    family_id: Uuid,
    scope: String,
}

/// Rotates a refresh token: the presented one is marked used and a new pair
/// from the same family is issued. Only a live token of a non-revoked
/// session of `client` is rotated; others are left untouched.
async fn refresh(
    db: &PgPool,
    client: &Client,
    request: &TokenRequest,
) -> Result<TokenResponse, OAuthError> {
    let token = request
        .refresh_token
        .as_deref()
        .ok_or_else(|| OAuthError::invalid_request("refresh_token is missing"))?;
    let token_hash = hash_token(token);

    let mut tx = db.begin().await?;
    let grant: Option<RefreshGrant> = sqlx::query_as(
        "UPDATE oauth_refresh_tokens r SET used_at = now()
         FROM agent_sessions s
         WHERE r.token_hash = $1 AND r.used_at IS NULL AND r.expires_at > now()
           AND s.id = r.agent_session_id AND s.client_id = $2 AND s.revoked_at IS NULL
         RETURNING r.agent_session_id, r.family_id, r.scope",
    )
    .bind(&token_hash)
    .bind(&client.client_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(grant) = grant else {
        tx.rollback().await?;
        revoke_family_if_reused(db, &token_hash).await?;
        return Err(OAuthError::invalid_grant(
            "refresh token is invalid or expired",
        ));
    };
    let tokens = issue(
        &mut *tx,
        grant.agent_session_id,
        &grant.scope,
        grant.family_id,
    )
    .await?;
    tx.commit().await?;
    Ok(tokens)
}

/// A refresh token presented after it was rotated may have been stolen, so
/// its whole family, access tokens included, is revoked (RFC 9700 §4.14.2).
async fn revoke_family_if_reused(db: &PgPool, token_hash: &[u8]) -> sqlx::Result<()> {
    let revoked: Option<Uuid> = sqlx::query_scalar(
        "WITH family AS (
             SELECT family_id FROM oauth_refresh_tokens
             WHERE token_hash = $1 AND used_at IS NOT NULL
         ), access AS (
             DELETE FROM oauth_access_tokens WHERE family_id IN (SELECT family_id FROM family)
         )
         DELETE FROM oauth_refresh_tokens WHERE family_id IN (SELECT family_id FROM family)
         RETURNING family_id",
    )
    .bind(token_hash)
    .fetch_optional(db)
    .await?;
    if let Some(family_id) = revoked {
        tracing::warn!(%family_id, "refresh token reuse detected; revoked its token family");
    }
    Ok(())
}

async fn issue(
    db: impl PgExecutor<'_>,
    session_id: Uuid,
    scope: &str,
    family_id: Uuid,
) -> sqlx::Result<TokenResponse> {
    let access_token = random_token();
    let refresh_token = random_token();
    sqlx::query(
        "WITH access AS (
             INSERT INTO oauth_access_tokens
                 (token_hash, agent_session_id, family_id, scope, expires_at)
             VALUES ($1, $3, $4, $5, now() + $6)
         )
         INSERT INTO oauth_refresh_tokens
             (token_hash, agent_session_id, family_id, scope, expires_at)
         VALUES ($2, $3, $4, $5, now() + $7)",
    )
    .bind(hash_token(&access_token))
    .bind(hash_token(&refresh_token))
    .bind(session_id)
    .bind(family_id)
    .bind(scope)
    .bind(ACCESS_TOKEN_TTL)
    .bind(REFRESH_TOKEN_TTL)
    .execute(db)
    .await?;
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_TTL.num_seconds(),
        refresh_token,
        scope: scope.to_owned(),
    })
}
