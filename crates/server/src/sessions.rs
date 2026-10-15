//! Agent sessions: one connected MCP client per user. Shared by the dashboard
//! API and the OAuth consent flow.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AgentSession {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    pub client_name: Option<String>,
    pub origin: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// Tool calls the session has made.
    pub call_count: i64,
}

/// A partial update from `PATCH /api/sessions/:id`; absent fields are left alone.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPatch {
    pub name: Option<String>,
}

/// Trims a user-supplied session name and checks it is reasonable.
pub fn normalize_name(name: &str) -> AppResult<String> {
    const MAX_CHARS: usize = 100;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_CHARS {
        return Err(AppError::BadRequest(format!(
            "name must be 1 to {MAX_CHARS} characters"
        )));
    }
    Ok(name.to_owned())
}

const SELECT_ACTIVE: &str = "
    SELECT s.id, s.name, s.client_id, c.client_name, s.origin, s.created_at, s.last_used_at,
           (SELECT count(*) FROM tool_calls t
             WHERE t.user_id = s.user_id AND t.agent_session_id = s.id) AS call_count
    FROM agent_sessions s
    JOIN oauth_clients c ON c.client_id = s.client_id
    WHERE s.user_id = $1 AND s.revoked_at IS NULL AND ($2::uuid IS NULL OR s.id = $2)
    ORDER BY s.created_at DESC";

/// The user's non-revoked sessions, newest first.
pub async fn list_active(db: &PgPool, user_id: Uuid) -> sqlx::Result<Vec<AgentSession>> {
    sqlx::query_as(SELECT_ACTIVE)
        .bind(user_id)
        .bind(None::<Uuid>)
        .fetch_all(db)
        .await
}

/// A non-revoked session owned by `user_id`.
pub async fn find(db: &PgPool, user_id: Uuid, id: Uuid) -> sqlx::Result<Option<AgentSession>> {
    sqlx::query_as(SELECT_ACTIVE)
        .bind(user_id)
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Applies `patch` to a non-revoked session. Returns whether it exists.
pub async fn update(
    db: &PgPool,
    user_id: Uuid,
    id: Uuid,
    patch: &SessionPatch,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE agent_sessions SET name = COALESCE($3, name)
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .bind(&patch.name)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Revokes a session and every credential issued to it. Its tool-call log is
/// kept. Returns whether an active session was revoked.
pub async fn revoke(db: &PgPool, user_id: Uuid, id: Uuid) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let revoked = sqlx::query(
        "UPDATE agent_sessions SET revoked_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if revoked {
        for sql in [
            "DELETE FROM oauth_access_tokens WHERE agent_session_id = $1",
            "DELETE FROM oauth_refresh_tokens WHERE agent_session_id = $1",
            "DELETE FROM oauth_codes WHERE agent_session_id = $1",
        ] {
            sqlx::query(sql).bind(id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(revoked)
}
