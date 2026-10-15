//! Dashboard cookie sessions and the [`CurrentUser`] extractor.

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts, State},
    http::{StatusCode, request::Parts},
};
use axum_extra::extract::{CookieJar, cookie::Cookie};
use chrono::{TimeDelta, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    config::{Config, username_allowed_sql},
    crypto::{hash_token, random_token},
    error::AppError,
    state::AppState,
};

pub const COOKIE: &str = "ilmcp_session";
const TTL_DAYS: i64 = 30;

/// The signed-in dashboard user. Rejects with `401` when the session cookie
/// is missing or expired, or the user was removed from
/// `ALLOWED_MOODLE_USERNAMES` since signing in; use `Option<CurrentUser>` to
/// make it optional. Serializes as the dashboard's `User`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CurrentUser {
    pub id: Uuid,
    pub moodle_userid: i64,
    pub username: String,
    pub fullname: String,
    pub avatar_url: Option<String>,
}

impl OptionalFromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, AppError> {
        let Some(cookie) = CookieJar::from_headers(&parts.headers).get(COOKIE).cloned() else {
            return Ok(None);
        };
        let user = sqlx::query_as(concat!(
            "SELECT u.id, u.moodle_userid, u.username, u.fullname, u.avatar_url
             FROM dashboard_sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now() AND ",
            username_allowed_sql!(),
        ))
        .bind(hash_token(cookie.value()))
        .bind(&state.config.allowed_moodle_usernames)
        .fetch_optional(&state.db)
        .await?;
        Ok(user)
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        <Self as OptionalFromRequestParts<AppState>>::from_request_parts(parts, state)
            .await?
            .ok_or(AppError::Unauthorized)
    }
}

/// Creates a dashboard session for `user_id` and returns its cookie.
pub async fn start(db: &PgPool, config: &Config, user_id: Uuid) -> sqlx::Result<Cookie<'static>> {
    let token = random_token();
    sqlx::query(
        "INSERT INTO dashboard_sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(hash_token(&token))
    .bind(user_id)
    .bind(Utc::now() + TimeDelta::days(TTL_DAYS))
    .execute(db)
    .await?;
    Ok(super::cookie(
        config,
        COOKIE,
        token,
        "/",
        time::Duration::days(TTL_DAYS),
    ))
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), AppError> {
    if let Some(cookie) = jar.get(COOKIE) {
        sqlx::query("DELETE FROM dashboard_sessions WHERE token_hash = $1")
            .bind(hash_token(cookie.value()))
            .execute(&state.db)
            .await?;
    }
    Ok((
        jar.remove(Cookie::build(COOKIE).path("/")),
        StatusCode::NO_CONTENT,
    ))
}
