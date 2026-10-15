//! `/api/sessions`: the connected agents.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use uuid::Uuid;

use crate::{
    auth::CurrentUser,
    error::{AppError, AppResult},
    sessions::{self, AgentSession, SessionPatch},
    state::AppState,
};

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<AgentSession>>> {
    Ok(Json(sessions::list_active(&state.db, user.id).await?))
}

pub async fn show(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<AgentSession>> {
    let session = sessions::find(&state.db, user.id, id).await?;
    session.map(Json).ok_or(AppError::NotFound)
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(mut patch): Json<SessionPatch>,
) -> AppResult<Json<AgentSession>> {
    patch.name = patch
        .name
        .as_deref()
        .map(sessions::normalize_name)
        .transpose()?;
    if !sessions::update(&state.db, user.id, id, &patch).await? {
        return Err(AppError::NotFound);
    }
    let session = sessions::find(&state.db, user.id, id).await?;
    session.map(Json).ok_or(AppError::NotFound)
}

pub async fn revoke(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    if !sessions::revoke(&state.db, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::testing::{self, dashboard_request, send};

    #[sqlx::test(migrations = "../../migrations")]
    async fn rename_and_revoke(db: PgPool) {
        let state = testing::state(db);
        let (user_id, cookie) = testing::sign_in(&state, 7).await;
        let session = testing::agent_session(&state, user_id).await;
        let app = crate::app(state.clone());
        let uri = format!("/api/sessions/{session}");
        let patch = |body: Value| {
            let app = app.clone();
            let request = dashboard_request(Method::PATCH, &uri, &cookie, Some(&body));
            async move { send(&app, request).await }
        };

        let updated = patch(json!({ "name": "  Renamed " })).await.json();
        assert_eq!(updated["name"], "Renamed");
        assert_eq!(updated["client_name"], "Test Agent");
        assert_eq!(updated["call_count"], 0);
        assert_eq!(
            patch(json!({ "name": " " })).await.status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            patch(json!({ "read_only": false })).await.status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        // Other users cannot see or change the session.
        let (_, other) = testing::sign_in(&state, 8).await;
        let request = dashboard_request(Method::PATCH, &uri, &other, Some(&json!({ "name": "x" })));
        assert_eq!(send(&app, request).await.status, StatusCode::NOT_FOUND);
        let request = dashboard_request(Method::GET, "/api/sessions", &other, None);
        assert_eq!(send(&app, request).await.json(), json!([]));

        let request = dashboard_request(Method::DELETE, &uri, &cookie, None);
        assert_eq!(send(&app, request).await.status, StatusCode::NO_CONTENT);
        let request = dashboard_request(Method::GET, "/api/sessions", &cookie, None);
        assert_eq!(send(&app, request).await.json(), json!([]));
    }
}
