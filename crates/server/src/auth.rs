//! Signing in by linking an iLearning account: the user opens Moodle's mobile
//! launch page, signs in with Microsoft 365 and pastes back the
//! `moodlemobile://token=…` link, which carries their web service token.
//!
//! `start` records an attempt (its sealed payload is the passport sent to
//! the launch page) bound to this browser by the `ilmcp_link` pre-auth
//! cookie. `complete` checks the pasted link against it, asks iLearning who
//! the token belongs to, consumes the attempt, upserts the user by Moodle
//! user ID, stores the tokens and starts a dashboard session.

use axum::{Json, extract::State, http::StatusCode};
use axum_extra::extract::{CookieJar, cookie::Cookie};
use chrono::{DateTime, TimeDelta, Utc};
use mcp_backbone::{
    AppError, AppResult, AppState,
    auth::{self, CurrentUser, Profile, attempts},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{links, service::ILearningService};

const COOKIE_PATH: &str = "/auth/ilearning";
const ATTEMPT_TTL: TimeDelta = TimeDelta::minutes(15);

type State_ = AppState<ILearningService>;

fn cookie_name(state: &State_) -> String {
    state.info.cookie_name("link")
}

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

/// `POST /auth/ilearning/start {next}`: a launch URL for this browser's
/// sign-in attempt.
pub async fn start(
    State(state): State<State_>,
    jar: CookieJar,
    body: Option<Json<StartRequest>>,
) -> AppResult<(CookieJar, Json<LinkStart>)> {
    let next = body.and_then(|Json(body)| body.next);
    let next = auth::local_path_or_root(next.as_deref());
    let passport = moodle::link::new_passport();
    let expires_at = Utc::now() + ATTEMPT_TTL;
    let attempt = attempts::start(
        &state,
        None,
        Some(passport.as_bytes()),
        Some(&next),
        ATTEMPT_TTL,
    )
    .await?;
    let moodle = &state.service.moodle;
    let launch_url = moodle::link::launch_url(moodle.wwwroot(), &passport, true);
    let cookie = auth::cookie(
        &state.config,
        cookie_name(&state),
        attempt.preauth,
        COOKIE_PATH,
        time::Duration::minutes(ATTEMPT_TTL.num_minutes()),
    );
    Ok((
        jar.add(cookie),
        Json(LinkStart {
            attempt_id: attempt.id,
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

fn expired() -> AppError {
    AppError::coded(
        StatusCode::GONE,
        "link_expired",
        "This sign-in attempt expired. Start again.",
    )
}

/// `POST /auth/ilearning/complete {attempt_id, pasted}`: signs in as the
/// owner of the pasted link's token and returns `{redirect_to}`.
pub async fn complete(
    State(state): State<State_>,
    jar: CookieJar,
    signed_in: Option<CurrentUser>,
    Json(body): Json<CompleteRequest>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let name = cookie_name(&state);
    let preauth = jar.get(&name).ok_or_else(expired)?.value().to_owned();
    // Peeked, not taken: a mistyped link may be pasted again.
    let attempt = attempts::peek(&state, body.attempt_id, &preauth)
        .await?
        .ok_or_else(expired)?;
    let passport = attempt
        .payload
        .and_then(|payload| String::from_utf8(payload).ok())
        .ok_or_else(|| anyhow::anyhow!("the sign-in attempt has no passport"))?;

    let moodle = &state.service.moodle;
    let tokens = moodle::link::parse(moodle.wwwroot(), &passport, &body.pasted).map_err(|err| {
        AppError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_link",
            format!("That is not the app link from this sign-in attempt: {err}"),
        )
    })?;
    let info = moodle
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

    let mut profile = Profile::new(info.userid.to_string());
    profile.login = Some(info.username.clone());
    profile.name = Some(info.fullname.clone());
    profile.avatar_url = info.userpictureurl.clone();
    if !auth::is_allowed(&state.config, &profile) {
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
        && user.subject != profile.subject
    {
        return Err(AppError::coded(
            StatusCode::CONFLICT,
            "wrong_account",
            format!(
                "You are signed in here as {} but linked the iLearning account {}. Sign out \
                 first to switch accounts.",
                user.login.as_deref().unwrap_or(&user.subject),
                info.username
            ),
        ));
    }

    // Consumed only now, once; a concurrent completion of the same attempt
    // that got here first wins.
    let attempt = attempts::take(&state, body.attempt_id, &preauth)
        .await?
        .ok_or_else(expired)?;
    let user_id = auth::upsert_user(&state.db, &profile).await?;
    links::store_linked(&state, &state.service.config, user_id, &tokens).await?;
    tracing::info!(%user_id, moodle_userid = info.userid, "linked iLearning account");

    let session = auth::start_session(&state, user_id).await?;
    let jar = jar
        .add(session)
        .remove(Cookie::build(name).path(COOKIE_PATH));
    let redirect_to = auth::local_path_or_root(attempt.next.as_deref());
    Ok((jar, Json(json!({ "redirect_to": redirect_to }))))
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode, header};
    use serde_json::{Value, json};
    use sqlx::PgPool;

    use crate::testing::{self, ILEARNING_URL, dashboard_request, send};

    fn cookie(response: &testing::Response) -> String {
        let set_cookie = response.headers[header::SET_COOKIE].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }

    #[sqlx::test(migrations = false)]
    async fn attempts_are_bound_to_the_browser(db: PgPool) {
        let state = testing::state(db.clone()).await;
        let app = mcp_backbone::router(state);
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
        assert!(attempt["expires_at"].is_string());
        let set_cookie = response.headers[header::SET_COOKIE].to_str().unwrap();
        assert!(set_cookie.starts_with("ilmcp_link="), "{set_cookie}");
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Path=/auth/ilearning"), "{set_cookie}");
        assert!(set_cookie.contains("Max-Age=900"), "{set_cookie}");
        let cookie = cookie(&response);

        let attempt_id: uuid::Uuid = attempt["attempt_id"].as_str().unwrap().parse().unwrap();
        let (next, sealed): (String, Vec<u8>) =
            sqlx::query_as("SELECT next, payload_sealed FROM link_attempts WHERE id = $1")
                .bind(attempt_id)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(next, "/consent/123");
        let passport = testing::passport(launch_url);
        assert!(
            !sealed
                .windows(passport.len())
                .any(|w| w == passport.as_bytes()),
            "the passport is sealed"
        );

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

        let other_cookie = cookie_of(start(Value::Null).await);
        let wrong_browser = complete(other_cookie, id.clone(), "x").await;
        assert_eq!(wrong_browser.status, StatusCode::GONE);
        assert_eq!(wrong_browser.json()["error"], "link_expired");

        let garbage = complete(cookie.clone(), id.clone(), "moodlemobile://token=bm9wZQ==").await;
        assert_eq!(garbage.status, StatusCode::BAD_REQUEST);
        let body = garbage.json();
        assert_eq!(body["error"], "invalid_link");
        assert!(body["message"].is_string());
        // A bad paste leaves the attempt for another try.
        let again = complete(cookie.clone(), id.clone(), "x").await;
        assert_eq!(again.json()["error"], "invalid_link");

        // An expired attempt is gone.
        sqlx::query("UPDATE link_attempts SET expires_at = now() - interval '1 second'")
            .execute(&db)
            .await
            .unwrap();
        let lapsed = complete(cookie.clone(), id.clone(), "x").await;
        assert_eq!(lapsed.status, StatusCode::GONE);

        // Cross-site requests never reach the handlers.
        let mut cross_site =
            dashboard_request(Method::POST, "/auth/ilearning/start", "", Some(&json!({})));
        cross_site
            .headers_mut()
            .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert_eq!(send(&app, cross_site).await.status, StatusCode::FORBIDDEN);
    }

    fn cookie_of(response: testing::Response) -> String {
        cookie(&response)
    }

    #[sqlx::test(migrations = false)]
    async fn next_must_stay_on_this_origin(db: PgPool) {
        let state = testing::state(db.clone()).await;
        let app = mcp_backbone::router(state);
        for (next, stored) in [
            (json!("//evil.example"), "/"),
            (json!("https://evil.example"), "/"),
            (json!("/\\evil.example"), "/"),
            (Value::Null, "/"),
            (json!("/sessions?x=1"), "/sessions?x=1"),
        ] {
            sqlx::query("DELETE FROM link_attempts")
                .execute(&db)
                .await
                .unwrap();
            let request = dashboard_request(
                Method::POST,
                "/auth/ilearning/start",
                "",
                Some(&json!({ "next": next })),
            );
            assert_eq!(send(&app, request).await.status, StatusCode::OK);
            let saved: String = sqlx::query_scalar("SELECT next FROM link_attempts")
                .fetch_one(&db)
                .await
                .unwrap();
            assert_eq!(saved, stored, "{next}");
        }
        // A body is optional.
        let request = dashboard_request(Method::POST, "/auth/ilearning/start", "", None);
        assert_eq!(send(&app, request).await.status, StatusCode::OK);
    }
}
