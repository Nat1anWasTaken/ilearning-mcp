//! `/api/tool-calls`: the call log.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::CurrentUser,
    error::{AppError, AppResult},
    mcp::{self, ToolCall, ToolCallFilter, ToolCallStatus},
    state::AppState,
};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;

#[derive(Deserialize)]
pub struct ListQuery {
    session_id: Option<Uuid>,
    status: Option<ToolCallStatus>,
    tool: Option<String>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct Page {
    items: Vec<ToolCall>,
    /// Pass as `before` for the next page; null on the last page.
    next_before: Option<DateTime<Utc>>,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Page>> {
    let filter = ToolCallFilter {
        session_id: query.session_id,
        status: query.status,
        tool: query.tool,
        before: query.before,
        limit: query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
    };
    let items = mcp::list_tool_calls(&state.db, user.id, &filter).await?;
    let next_before = match items.last() {
        Some(last) if items.len() as i64 == filter.limit => Some(last.created_at),
        _ => None,
    };
    Ok(Json(Page { items, next_before }))
}

pub async fn show(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ToolCall>> {
    let call = mcp::find_tool_call(&state.db, user.id, id).await?;
    call.map(Json).ok_or(AppError::NotFound)
}
