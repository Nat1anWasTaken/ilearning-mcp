//! Signing in by linking an iLearning account: the user opens Moodle's mobile
//! launch page, signs in with Microsoft 365 and pastes back the
//! `moodlemobile://token=…` link, which carries their web service token.

use axum::{Json, extract::State, http::StatusCode};
use axum_extra::extract::{CookieJar, cookie::Cookie};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use super::{CurrentUser, session};
use crate::{
    crypto::{hash_token, random_token},
    error::{AppError, AppResult},
    links,
    state::AppState,
};

const COOKIE: &str = "ilmcp_link";
const COOKIE_PATH: &str = "/auth/ilearning";
const ATTEMPT_TTL: TimeDelta = TimeDelta::minutes(15);
const MAX_NEXT_CHARS: usize = 2048;

#[derive(Deserialize)]
pub struct StartRequest {
    next: Option<String>,
}

#[derive(Serialize)]
pub struct LinkStart {
    attempt_id: Uuid,
    launch_url: String,
    expires_at: DateTime<Utc>,
}

pub async fn start(
    State(state): State<AppState>,
    jar: CookieJar,
    body: Option<Json<StartRequest>>,
) -> AppResult<(CookieJar, Json<LinkStart>)> {
    let next = body
        .and_then(|Json(body)| body.next)
        .filter(|next| is_local_path(next));
    let passport = moodle::link::new_passport();
    let preauth = random_token();
    let expires_at = Utc::now() + ATTEMPT_TTL;
    let attempt_id: Uuid = sqlx::query_scalar(
        "INSERT INTO link_attempts (passport, preauth_hash, next, expires_at)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(&passport)
    .bind(hash_token(&preauth))
    .bind(next)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await?;
    let launch_url = moodle::link::launch_url(state.moodle.wwwroot(), &passport, true);
    let cookie = super::cookie(
        &state.config,
        COOKIE,
        preauth,
        COOKIE_PATH,
        time::Duration::minutes(ATTEMPT_TTL.num_minutes()),
    );
    Ok((
        jar.add(cookie),
        Json(LinkStart {
            attempt_id,
            launch_url: launch_url.to_string(),
            expires_at,
        }),
    ))
}

#[derive(Deserialize)]
pub struct CompleteRequest {
    attempt_id: Uuid,
    pasted: String,
}

#[derive(FromRow)]
struct Attempt {
    passport: String,
    next: Option<String>,
}

pub async fn complete(
    State(state): State<AppState>,
    jar: CookieJar,
    signed_in: Option<CurrentUser>,
    Json(body): Json<CompleteRequest>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let expired = || {
        AppError::coded(
            StatusCode::GONE,
            "link_expired",
            "This sign-in attempt expired. Start again.",
        )
    };
    let preauth = jar.get(COOKIE).ok_or_else(expired)?.value().to_owned();
    let attempt: Attempt = sqlx::query_as(
        "SELECT passport, next FROM link_attempts
         WHERE id = $1 AND preauth_hash = $2 AND expires_at > now()",
    )
    .bind(body.attempt_id)
    .bind(hash_token(&preauth))
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(expired)?;

    let tokens = moodle::link::parse(state.moodle.wwwroot(), &attempt.passport, &body.pasted)
        .map_err(|err| {
            AppError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_link",
                format!("That is not the app link from this sign-in attempt: {err}"),
            )
        })?;
    let info = state
        .moodle
        .account(tokens.token.clone())
        .site_info()
        .await
        .map_err(|err| match err {
            moodle::Error::InvalidToken => AppError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_link",
                "iLearning does not accept the token in that link. Start again.",
            ),
            other => AppError::coded(
                StatusCode::BAD_GATEWAY,
                "moodle_error",
                format!("Could not reach iLearning: {other}"),
            ),
        })?;
    if !state.config.is_username_allowed(&info.username) {
        return Err(AppError::coded(
            StatusCode::FORBIDDEN,
            "not_allowed",
            format!(
                "The iLearning account {} may not use this server.",
                info.username
            ),
        ));
    }
    if let Some(user) = &signed_in
        && user.moodle_userid != info.userid
    {
        return Err(AppError::coded(
            StatusCode::CONFLICT,
            "wrong_account",
            format!(
                "You are signed in here as {} but linked the iLearning account {}. Sign out \
                 first to switch accounts.",
                user.username, info.username
            ),
        ));
    }

    let mut tx = state.db.begin().await?;
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (moodle_userid, username, fullname, avatar_url)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (moodle_userid) DO UPDATE SET
             username = EXCLUDED.username,
             fullname = EXCLUDED.fullname,
             avatar_url = EXCLUDED.avatar_url,
             last_login_at = now()
         RETURNING id",
    )
    .bind(info.userid)
    .bind(&info.username)
    .bind(&info.fullname)
    .bind(&info.userpictureurl)
    .fetch_one(&mut *tx)
    .await?;
    links::store_linked(&mut tx, &state.config, user_id, &tokens).await?;
    let deleted = sqlx::query("DELETE FROM link_attempts WHERE id = $1")
        .bind(body.attempt_id)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() != 1 {
        // A concurrent completion of the same attempt won.
        return Err(expired());
    }
    tx.commit().await?;
    tracing::info!(%user_id, moodle_userid = info.userid, "linked iLearning account");

    let session = session::start(&state.db, &state.config, user_id).await?;
    let jar = jar
        .add(session)
        .remove(Cookie::build(COOKIE).path(COOKIE_PATH));
    let redirect_to = attempt.next.unwrap_or_else(|| "/".to_owned());
    Ok((jar, Json(json!({ "redirect_to": redirect_to }))))
}

/// Whether `next` is a path on this origin, so redirecting to it after sign-in
/// cannot send the browser elsewhere.
fn is_local_path(next: &str) -> bool {
    next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && !next.chars().any(char::is_control)
        && next.len() <= MAX_NEXT_CHARS
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode, header};
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use super::is_local_path;
    use crate::testing::{self, ILEARNING_URL, dashboard_request, send};

    #[test]
    fn next_must_stay_on_this_origin() {
        assert!(is_local_path("/"));
        assert!(is_local_path("/oauth/authorize?client_id=x&state=y"));
        assert!(!is_local_path("//evil.example"));
        assert!(!is_local_path("/\\evil.example"));
        assert!(!is_local_path("https://evil.example"));
        assert!(!is_local_path("evil"));
        assert!(!is_local_path("/a\nb"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn attempts_are_bound_to_the_browser(db: PgPool) {
        let state = testing::state(db);
        let app = crate::app(state);
        let start = |next: Value| {
            let app = app.clone();
            async move {
                let request = dashboard_request(
                    Method::POST,
                    "/auth/ilearning/start",
                    "",
                    Some(&json!({ "next": next })),
                );
                send(&app, request).await
            }
        };

        let response = start(json!("/consent/123")).await;
        assert_eq!(response.status, StatusCode::OK);
        let attempt = response.json();
        let launch_url = attempt["launch_url"].as_str().unwrap();
        assert!(
            launch_url.starts_with(&format!("{ILEARNING_URL}admin/tool/mobile/launch.php?")),
            "{launch_url}"
        );
        let set_cookie = response.headers[header::SET_COOKIE].to_str().unwrap();
        assert!(set_cookie.starts_with("ilmcp_link="), "{set_cookie}");
        assert!(set_cookie.contains("HttpOnly"));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();

        let complete = |cookie: String, attempt_id: Value, pasted: &'static str| {
            let app = app.clone();
            async move {
                let body = json!({ "attempt_id": attempt_id, "pasted": pasted });
                let request = dashboard_request(
                    Method::POST,
                    "/auth/ilearning/complete",
                    &cookie,
                    Some(&body),
                );
                send(&app, request).await
            }
        };
        let id = attempt["attempt_id"].clone();

        let no_cookie = complete(String::new(), id.clone(), "x").await;
        assert_eq!(no_cookie.status, StatusCode::GONE);
        assert_eq!(no_cookie.json()["error"], "link_expired");

        let other_cookie = start(Value::Null).await;
        let other_cookie = other_cookie.headers[header::SET_COOKIE].to_str().unwrap();
        let other_cookie = other_cookie.split(';').next().unwrap().to_owned();
        let wrong_browser = complete(other_cookie, id.clone(), "x").await;
        assert_eq!(wrong_browser.json()["error"], "link_expired");

        let garbage = complete(cookie.clone(), id.clone(), "moodlemobile://token=bm9wZQ==").await;
        assert_eq!(garbage.status, StatusCode::BAD_REQUEST);
        let body = garbage.json();
        assert_eq!(body["error"], "invalid_link");
        assert!(body["message"].is_string());

        // Cross-site requests never reach the handlers.
        let mut cross_site =
            dashboard_request(Method::POST, "/auth/ilearning/start", "", Some(&json!({})));
        cross_site
            .headers_mut()
            .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert_eq!(send(&app, cross_site).await.status, StatusCode::FORBIDDEN);
    }
}
