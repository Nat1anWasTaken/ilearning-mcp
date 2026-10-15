//! The dashboard JSON API, mounted at `/api`.

mod sessions;
mod tool_calls;
mod tools;

use axum::{
    Json, Router,
    body::HttpBody,
    extract::{Request, State},
    http::{HeaderMap, header},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use serde_json::{Value, json};

use crate::{
    auth::CurrentUser,
    error::{AppError, AppResult},
    links,
    oauth::consent,
    state::AppState,
};

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/me", get(me))
        .route("/tools", get(tools::list))
        .route("/tool-calls", get(tool_calls::list))
        .route("/tool-calls/{id}", get(tool_calls::show))
        .route("/sessions", get(sessions::list))
        .route(
            "/sessions/{id}",
            get(sessions::show)
                .patch(sessions::update)
                .delete(sessions::revoke),
        )
        .route(
            "/oauth/requests/{id}",
            get(consent::show).post(consent::decide),
        )
        .fallback(|| async { AppError::NotFound })
        .layer(middleware::from_fn_with_state(state, csrf_guard))
}

async fn me(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Value>> {
    let link = links::status(&state.db, user.id).await?;
    Ok(Json(json!({ "user": user, "link": link })))
}

/// CSRF defence alongside `SameSite=Lax` cookies: mutating requests must come
/// from our own origin (when the browser says) and carry JSON, which a
/// cross-site form cannot send.
pub async fn csrf_guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    if !request.method().is_safe() {
        let has_body = !request.body().is_end_stream();
        let headers = request.headers();
        if headers
            .get(header::ORIGIN)
            .is_some_and(|origin| !origin.to_str().is_ok_and(|o| state.config.is_own_origin(o)))
        {
            return Err(AppError::Forbidden("cross-origin request rejected".into()));
        }
        if has_body && !is_json(headers) {
            return Err(AppError::BadRequest(
                "request body must be application/json".into(),
            ));
        }
    }
    Ok(next.run(request).await)
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use serde_json::json;
    use sqlx::PgPool;

    use crate::testing::{self, dashboard_request, send};

    #[sqlx::test(migrations = "../../migrations")]
    async fn me_requires_a_session(db: PgPool) {
        let state = testing::state(db);
        let (user_id, cookie) = testing::sign_in(&state, 7).await;
        let app = crate::app(state);

        let response = send(
            &app,
            dashboard_request(Method::GET, "/api/me", &cookie, None),
        )
        .await;
        assert_eq!(
            response.json(),
            json!({
                "user": {
                    "id": user_id,
                    "moodle_userid": 7,
                    "username": "user7",
                    "fullname": "User 7",
                    "avatar_url": null,
                },
                "link": {
                    "status": "ok",
                    "token_obtained_at": response.json()["link"]["token_obtained_at"],
                    "expires_at": null,
                    "last_ok_at": response.json()["link"]["last_ok_at"],
                    "last_error": null,
                    "auto_renew": false,
                },
            })
        );

        let anonymous = send(&app, Request::get("/api/me").body(Body::empty()).unwrap()).await;
        assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
        assert_eq!(anonymous.json()["error"], "unauthorized");
        let stale = dashboard_request(Method::GET, "/api/me", "ilmcp_session=bogus", None);
        assert_eq!(send(&app, stale).await.status, StatusCode::UNAUTHORIZED);

        let unknown = send(
            &app,
            dashboard_request(Method::GET, "/api/nope", &cookie, None),
        )
        .await;
        assert_eq!(unknown.status, StatusCode::NOT_FOUND);
        assert_eq!(unknown.json()["error"], "not_found");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn csrf_guard_rejects_cross_origin_and_non_json(db: PgPool) {
        let state = testing::state(db);
        let (user_id, cookie) = testing::sign_in(&state, 7).await;
        let session = testing::agent_session(&state, user_id).await;
        let app = crate::app(state);
        let uri = format!("/api/sessions/{session}");
        let body = json!({ "name": "Renamed" });

        let mut cross_origin = dashboard_request(Method::PATCH, &uri, &cookie, Some(&body));
        cross_origin
            .headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        assert_eq!(send(&app, cross_origin).await.status, StatusCode::FORBIDDEN);

        let form = Request::patch(&uri)
            .header("cookie", &cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("name=Renamed"))
            .unwrap();
        assert_eq!(send(&app, form).await.status, StatusCode::BAD_REQUEST);

        // Bodyless deletes and same-origin JSON pass.
        let ok = send(
            &app,
            dashboard_request(Method::PATCH, &uri, &cookie, Some(&body)),
        )
        .await;
        assert_eq!(ok.status, StatusCode::OK);
        let delete = send(&app, dashboard_request(Method::DELETE, &uri, &cookie, None)).await;
        assert_eq!(delete.status, StatusCode::NO_CONTENT);
    }
}
