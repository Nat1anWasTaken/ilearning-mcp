//! The `tool_calls` table: every MCP tool call, as the dashboard shows it.
//!
//! Arguments and results are stored bounded: long strings are cut, file
//! bodies (`base64` fields) are replaced by their length, and oversized JSON
//! is replaced by a marked preview.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::oauth::AgentPrincipal;

const MAX_STRING_CHARS: usize = 2000;
/// Strings of arguments that did not parse may be anything, such as file
/// content; longer ones are logged by length only.
const MAX_RAW_STRING_CHARS: usize = 256;
const MAX_JSON_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Status {
    Running,
    Ok,
    Error,
}

/// A logged call, as served by `/api/tool-calls`.
#[derive(Debug, Serialize, FromRow)]
pub struct ToolCall {
    pub id: Uuid,
    pub session_id: Uuid,
    pub session_name: String,
    pub tool: String,
    pub arguments: Value,
    pub status: Status,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub duration_ms: Option<i32>,
    pub created_at: DateTime<Utc>,
}

/// The parts of a call known before it runs.
pub struct NewCall {
    user_id: Uuid,
    session_id: Uuid,
    tool: &'static str,
    arguments: Value,
}

impl NewCall {
    /// A call with its parsed arguments, as the request logs them.
    pub fn new(principal: AgentPrincipal, tool: &'static str, arguments: &Value) -> Self {
        Self::with(principal, tool, redact(arguments, truncate))
    }

    /// A call whose arguments did not parse.
    pub fn unparsed(principal: AgentPrincipal, tool: &'static str, arguments: &Value) -> Self {
        let arguments = redact(arguments, |text| match text.chars().count() {
            ..=MAX_RAW_STRING_CHARS => text.to_owned(),
            chars => format!("[{chars} characters]"),
        });
        Self::with(principal, tool, arguments)
    }

    fn with(principal: AgentPrincipal, tool: &'static str, arguments: Value) -> Self {
        let text = arguments.to_string();
        Self {
            user_id: principal.user_id,
            session_id: principal.session_id,
            tool,
            arguments: bounded(arguments, &text),
        }
    }
}

/// Filters for [`list`].
pub struct Filter {
    pub session_id: Option<Uuid>,
    pub status: Option<Status>,
    pub tool: Option<String>,
    pub before: Option<DateTime<Utc>>,
    pub limit: i64,
}

/// A query for [`ToolCall`]s from `$source`, a table or CTE aliased as `t`.
macro_rules! select_calls {
    ($with:literal, $source:literal, $clauses:literal) => {
        concat!(
            $with,
            " SELECT t.id, t.agent_session_id AS session_id, s.name AS session_name, t.tool,
                     t.arguments, t.status, t.result, t.error, t.duration_ms, t.created_at
              FROM ",
            $source,
            " t JOIN agent_sessions s ON s.id = t.agent_session_id ",
            $clauses
        )
    };
}

/// Logs a call that ended before running, e.g. because its arguments were invalid.
pub async fn record(
    db: &PgPool,
    call: &NewCall,
    error: &str,
    duration: Duration,
) -> sqlx::Result<()> {
    insert(db, call, Status::Error, Some((error, duration))).await?;
    Ok(())
}

/// Logs a call that is about to run.
pub async fn start(db: &PgPool, call: &NewCall) -> sqlx::Result<Uuid> {
    insert(db, call, Status::Running, None).await
}

async fn insert(
    db: &PgPool,
    call: &NewCall,
    status: Status,
    ended: Option<(&str, Duration)>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO tool_calls (user_id, agent_session_id, tool, arguments, status, error,
                                 duration_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(call.user_id)
    .bind(call.session_id)
    .bind(call.tool)
    .bind(&call.arguments)
    .bind(status)
    .bind(ended.map(|(error, _)| error))
    .bind(ended.map(|(_, duration)| millis(duration)))
    .fetch_one(db)
    .await
}

/// Records how a running call ended. `result` must already be bounded.
pub async fn finish(
    db: &PgPool,
    id: Uuid,
    status: Status,
    result: Option<Value>,
    error: Option<&str>,
    duration: Duration,
) -> sqlx::Result<ToolCall> {
    sqlx::query_as(select_calls!(
        "WITH updated AS (
             UPDATE tool_calls SET status = $2, result = $3, error = $4, duration_ms = $5
             WHERE id = $1 RETURNING *
         )",
        "updated",
        ""
    ))
    .bind(id)
    .bind(status)
    .bind(result)
    .bind(error)
    .bind(millis(duration))
    .fetch_one(db)
    .await
}

/// Settles calls a previous process left running.
pub async fn settle_interrupted(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE tool_calls SET status = 'error', error = 'interrupted by a server restart'
         WHERE status = 'running'",
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn find(db: &PgPool, user_id: Uuid, id: Uuid) -> sqlx::Result<Option<ToolCall>> {
    sqlx::query_as(select_calls!(
        "",
        "tool_calls",
        "WHERE t.user_id = $1 AND t.id = $2"
    ))
    .bind(user_id)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// The user's calls, newest first.
pub async fn list(db: &PgPool, user_id: Uuid, filter: &Filter) -> sqlx::Result<Vec<ToolCall>> {
    sqlx::query_as(select_calls!(
        "",
        "tool_calls",
        "WHERE t.user_id = $1 AND ($2::uuid IS NULL OR t.agent_session_id = $2)
           AND ($3::text IS NULL OR t.status = $3)
           AND ($4::text IS NULL OR t.tool = $4)
           AND ($5::timestamptz IS NULL OR t.created_at < $5)
         ORDER BY t.created_at DESC
         LIMIT $6"
    ))
    .bind(user_id)
    .bind(filter.session_id)
    .bind(filter.status)
    .bind(&filter.tool)
    .bind(filter.before)
    .bind(filter.limit)
    .fetch_all(db)
    .await
}

fn millis(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

/// A copy of `value` with every string replaced by `string(it)`, except file
/// bodies (strings under a `base64` key), which only keep their length.
fn redact(value: &Value, string: impl Fn(&str) -> String + Copy) -> Value {
    match value {
        Value::String(text) => Value::String(string(text)),
        Value::Array(items) => items.iter().map(|item| redact(item, string)).collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    Value::String(body) if key == "base64" => {
                        Value::String(format!("[{} characters]", body.chars().count()))
                    }
                    other => redact(other, string),
                };
                (key.clone(), value)
            })
            .collect(),
        other => other.clone(),
    }
}

fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_STRING_CHARS) {
        Some((end, _)) => format!(
            "{}… [{} characters in total]",
            &text[..end],
            text.chars().count()
        ),
        None => text.to_owned(),
    }
}

/// `value`, or a marked preview of `rendered` (any serialization of it) if
/// that is longer than [`MAX_JSON_BYTES`].
pub fn bounded(value: Value, rendered: &str) -> Value {
    if rendered.len() <= MAX_JSON_BYTES {
        return value;
    }
    json!({
        "truncated": true,
        "bytes": rendered.len(),
        "preview": &rendered[..rendered.floor_char_boundary(MAX_JSON_BYTES)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> AgentPrincipal {
        AgentPrincipal {
            user_id: Uuid::nil(),
            session_id: Uuid::nil(),
        }
    }

    #[test]
    fn cuts_long_strings() {
        let long = "é".repeat(MAX_STRING_CHARS + 5);
        let arguments = json!({
            "assignment_id": 3,
            "online_text": long,
            "files": [{ "filename": "a.txt", "base64": "QUJD".repeat(10) }],
        });
        let redacted = NewCall::new(principal(), "submit_assignment", &arguments).arguments;
        assert_eq!(redacted["assignment_id"], 3);
        assert_eq!(
            redacted["files"],
            json!([{ "filename": "a.txt", "base64": "[40 characters]" }])
        );
        let text = redacted["online_text"].as_str().unwrap();
        assert!(text.starts_with(&"é".repeat(MAX_STRING_CHARS)));
        assert!(text.ends_with("… [2005 characters in total]"));
    }

    #[test]
    fn logs_unparsed_arguments_conservatively() {
        let arguments = json!({
            "post_id": "me",
            "message": "y".repeat(MAX_RAW_STRING_CHARS + 1),
            "list": ["x".repeat(MAX_RAW_STRING_CHARS), 7],
        });
        let logged = NewCall::unparsed(principal(), "reply_forum_post", &arguments).arguments;
        assert_eq!(
            logged,
            json!({
                "post_id": "me",
                "message": "[257 characters]",
                "list": ["x".repeat(MAX_RAW_STRING_CHARS), 7],
            })
        );
    }

    #[test]
    fn bounds_large_json() {
        let small = json!({ "id": 1 });
        assert_eq!(bounded(small.clone(), &small.to_string()), small);

        let large = json!(vec!["ü".repeat(100); 200]);
        let rendered = large.to_string();
        let bounded = bounded(large, &rendered);
        assert_eq!(bounded["truncated"], true);
        assert_eq!(bounded["bytes"], rendered.len());
        let preview = bounded["preview"].as_str().unwrap();
        assert!(preview.len() <= MAX_JSON_BYTES && preview.len() > MAX_JSON_BYTES - 4);
    }
}
