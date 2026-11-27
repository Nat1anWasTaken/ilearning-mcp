//! The app driven over HTTP against a test database: the `/mcp` endpoint,
//! the sign-in link flow and file downloads, with [`MockMoodle`] standing
//! in for iLearning where a call has to reach it.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use mcp_backbone::links::{LinkState, LinkStore};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    links,
    testing::{
        self, MockMoodle, PUBLIC_URL, Response, State_, access_token, agent_session, app_link,
        dashboard_request, mcp_request, send,
    },
};

struct Harness {
    state: State_,
    app: Router,
    user_id: Uuid,
    cookie: String,
    token: String,
}

impl Harness {
    async fn new(state: State_) -> Self {
        let (user_id, cookie) = testing::sign_in(&state, 7).await;
        let session_id = agent_session(&state, user_id).await;
        let token = access_token(&state, session_id).await;
        Self {
            app: mcp_backbone::router(state.clone()),
            state,
            user_id,
            cookie,
            token,
        }
    }

    /// A JSON-RPC request; returns the whole response message.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let response = send(&self.app, mcp_request(&self.token, method, params)).await;
        assert_eq!(response.status, StatusCode::OK, "{method}");
        response.json()
    }

    /// Calls a tool; returns its `CallToolResult`.
    async fn call(&self, tool: &str, arguments: Value) -> Value {
        let params = json!({ "name": tool, "arguments": arguments });
        self.rpc("tools/call", params).await["result"].clone()
    }

    async fn link(&self) -> links::LinkStatus {
        links::status(&self.state, self.user_id)
            .await
            .unwrap()
            .unwrap()
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

#[sqlx::test(migrations = false)]
async fn initialize_and_tools_list(db: PgPool) {
    let harness = Harness::new(testing::state(db).await).await;
    let initialize = harness
        .rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        )
        .await;
    let info = &initialize["result"];
    assert_eq!(info["serverInfo"]["name"], "ilearning");
    assert_eq!(info["serverInfo"]["title"], "iLearning");
    assert_eq!(
        info["serverInfo"]["icons"][0]["src"],
        format!("{PUBLIC_URL}/ilearning.png")
    );
    assert!(
        info["instructions"]
            .as_str()
            .unwrap()
            .contains("submit_assignment")
    );

    let listed = harness.rpc("tools/list", json!({})).await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 20);
    let tool = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap();
    assert_eq!(tool("list_courses")["annotations"]["readOnlyHint"], true);
    assert_eq!(
        tool("submit_assignment")["annotations"]["destructiveHint"],
        true
    );
    assert_eq!(
        tool("send_message")["annotations"]["destructiveHint"],
        false
    );
    assert_eq!(tool("send_message")["inputSchema"]["type"], "object");
}

#[sqlx::test(migrations = false)]
async fn file_bodies_are_logged_by_length(db: PgPool) {
    let harness = Harness::new(testing::state(db).await).await;
    let content = "QUJD".repeat(1000);
    // The site is unreachable, so the call fails at iLearning, but is logged.
    let result = harness
        .call(
            "submit_assignment",
            json!({ "assignment_id": 3, "files": [{ "filename": "a.bin", "base64": content }] }),
        )
        .await;
    assert_eq!(result["isError"], true);
    let page = send(
        &harness.app,
        dashboard_request(Method::GET, "/api/tool-calls", &harness.cookie, None),
    )
    .await
    .json();
    let call = &page["items"][0];
    assert_eq!(call["tool"], "submit_assignment");
    assert_eq!(call["status"], "error");
    assert_eq!(
        call["arguments"]["files"],
        json!([{ "filename": "a.bin", "base64": "[4000 characters]", "text": null }])
    );
}

#[sqlx::test(migrations = false)]
async fn unlinked_users_are_told_to_relink(db: PgPool) {
    let harness = Harness::new(testing::state(db).await).await;
    LinkStore::new(&harness.state)
        .delete(harness.user_id)
        .await
        .unwrap();
    let result = harness.call("get_profile", json!({})).await;
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("re-link"), "{result}");
    assert!(text(&result).contains(PUBLIC_URL));
}

/// iLearning's verdict on the token is recorded on the link, and a flagged
/// link is still tried: iLearning has the final word.
#[sqlx::test(migrations = false)]
async fn calls_reach_ilearning_and_record_its_verdict(db: PgPool) {
    let moodle = MockMoodle::start().await;
    let harness = Harness::new(moodle.state(db).await).await;
    let token = testing::token(7);
    moodle.accept(&token, 7);
    sqlx::query("UPDATE links SET last_ok_at = NULL")
        .execute(&harness.state.db)
        .await
        .unwrap();

    let profile = harness.call("get_profile", json!({})).await;
    assert_eq!(profile["isError"], false, "{profile}");
    assert_eq!(profile["structuredContent"]["username"], "s7");
    assert!(harness.link().await.last_ok_at.is_some());

    moodle.revoke(&token);
    let rejected = harness.call("get_profile", json!({})).await;
    assert_eq!(rejected["isError"], true);
    assert!(text(&rejected).contains("re-link"), "{rejected}");
    let flagged = harness.link().await;
    assert_eq!(flagged.status, LinkState::NeedsRelink);

    moodle.accept(&token, 7);
    let again = harness.call("get_profile", json!({})).await;
    assert_eq!(again["isError"], false, "{again}");
}

#[sqlx::test(migrations = false)]
async fn file_links_stream_the_file_while_the_token_works(db: PgPool) {
    let moodle = MockMoodle::start().await;
    let harness = Harness::new(moodle.state(db).await).await;
    let token = testing::token(7);
    moodle.accept(&token, 7);
    let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();

    let bogus = send(&harness.app, get("/files/bogus.link")).await;
    assert_eq!(bogus.status, StatusCode::FORBIDDEN);

    let signer = crate::files::Signer::new(&harness.state.config, harness.user_id);
    let link = signer.link(&format!(
        "{}pluginfile.php/1/mod_resource/content/0/notes.txt",
        moodle.url
    ));
    let path = link.strip_prefix(PUBLIC_URL).unwrap().to_owned();
    let ok = send(&harness.app, get(&path)).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.body, b"hello");
    assert_eq!(ok.headers[header::CONTENT_TYPE], "text/plain");
    assert!(
        ok.headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("notes.txt")
    );
    assert_eq!(ok.headers[header::CACHE_CONTROL], "private, no-store");

    moodle.revoke(&token);
    let gone = send(&harness.app, get(&path)).await;
    assert_eq!(gone.status, StatusCode::GONE);
    assert_eq!(harness.link().await.status, LinkState::NeedsRelink);

    LinkStore::new(&harness.state)
        .delete(harness.user_id)
        .await
        .unwrap();
    assert_eq!(
        send(&harness.app, get(&path)).await.status,
        StatusCode::NOT_FOUND
    );
}

fn set_cookie(response: &Response, name: &str) -> Option<String> {
    response
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_owned()
}

/// Starts an attempt; returns its ID, launch URL and pre-auth cookie.
async fn start(app: &Router, cookie: &str, next: &str) -> (Value, String, String) {
    let request = dashboard_request(
        Method::POST,
        "/auth/ilearning/start",
        cookie,
        Some(&json!({ "next": next })),
    );
    let response = send(app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    let preauth = cookie_pair(&set_cookie(&response, "ilmcp_link").unwrap());
    (
        body["attempt_id"].clone(),
        body["launch_url"].as_str().unwrap().to_owned(),
        preauth,
    )
}

async fn complete(app: &Router, cookie: &str, attempt_id: &Value, pasted: &str) -> Response {
    let body = json!({ "attempt_id": attempt_id, "pasted": pasted });
    let request = dashboard_request(
        Method::POST,
        "/auth/ilearning/complete",
        cookie,
        Some(&body),
    );
    send(app, request).await
}

#[sqlx::test(migrations = false)]
async fn linking_signs_in_as_the_tokens_owner(db: PgPool) {
    let moodle = MockMoodle::start().await;
    let state = moodle.state(db.clone()).await;
    let app = mcp_backbone::router(state.clone());
    let token = "a".repeat(32);
    moodle.accept(&token, 42);

    let (attempt_id, launch_url, preauth) = start(&app, "", "/sessions?x=1").await;
    let pasted = app_link(&moodle.url, &launch_url, &token, Some("privatetoken42"));
    let response = complete(&app, &preauth, &attempt_id, &pasted).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.json());
    assert_eq!(response.json()["redirect_to"], "/sessions?x=1");
    let cleared = set_cookie(&response, "ilmcp_link").unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    let session = cookie_pair(&set_cookie(&response, "ilmcp_session").unwrap());

    let me = send(
        &app,
        dashboard_request(Method::GET, "/api/me", &session, None),
    )
    .await
    .json();
    assert_eq!(me["user"]["subject"], "42");
    assert_eq!(me["user"]["login"], "s42");
    assert_eq!(me["user"]["name"], "Student 42");
    assert_eq!(me["user"]["avatar_url"], "https://ilearning.example/pic/42");
    let link = &me["link"];
    assert_eq!(link["status"], "ok");
    assert_eq!(
        link["expires_at"],
        Value::Null,
        "a first link's expiry is unknown"
    );
    assert_eq!(link["auto_renew"], false);
    assert!(link["token_obtained_at"].is_string() && link["last_ok_at"].is_string());
    assert_eq!(link["last_error"], Value::Null);
    let keys: Vec<_> = link.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "auto_renew",
            "expires_at",
            "last_error",
            "last_ok_at",
            "status",
            "token_obtained_at"
        ]
    );

    // The attempt is single use.
    let reused = complete(&app, &preauth, &attempt_id, &pasted).await;
    assert_eq!(reused.status, StatusCode::GONE);

    // Linking again while signed in, with a fresh token: its expiry is known.
    let fresh = "b".repeat(32);
    moodle.accept(&fresh, 42);
    let (attempt_id, launch_url, preauth) = start(&app, &session, "/").await;
    let pasted = app_link(&moodle.url, &launch_url, &fresh, None);
    let cookies = format!("{session}; {preauth}");
    let relinked = complete(&app, &cookies, &attempt_id, &pasted).await;
    assert_eq!(relinked.status, StatusCode::OK);
    let me = send(
        &app,
        dashboard_request(Method::GET, "/api/me", &session, None),
    )
    .await
    .json();
    assert!(me["link"]["expires_at"].is_string());
    assert_eq!(me["link"]["auto_renew"], true, "the private token is kept");
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(users, 1);

    // Another account's link while signed in is refused.
    let other = "c".repeat(32);
    moodle.accept(&other, 43);
    let (attempt_id, launch_url, preauth) = start(&app, &session, "/").await;
    let pasted = app_link(&moodle.url, &launch_url, &other, None);
    let cookies = format!("{session}; {preauth}");
    let wrong = complete(&app, &cookies, &attempt_id, &pasted).await;
    assert_eq!(wrong.status, StatusCode::CONFLICT);
    assert_eq!(wrong.json()["error"], "wrong_account");
    assert!(wrong.json()["message"].as_str().unwrap().contains("s42"));

    // A token iLearning refuses.
    let (attempt_id, launch_url, preauth) = start(&app, "", "/").await;
    let pasted = app_link(&moodle.url, &launch_url, &"d".repeat(32), None);
    let refused = complete(&app, &preauth, &attempt_id, &pasted).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(refused.json()["error"], "invalid_link");
    // A link minted for another attempt.
    let (attempt_id, _, preauth) = start(&app, "", "/").await;
    let stale = complete(&app, &preauth, &attempt_id, &pasted).await;
    assert_eq!(stale.json()["error"], "invalid_link");
}

#[sqlx::test(migrations = false)]
async fn the_allowlist_and_unreachable_sites(db: PgPool) {
    let moodle = MockMoodle::start().await;
    let mut state = moodle.state(db.clone()).await;
    let mut config = (*state.config).clone();
    config.allowed_users = vec!["s42".into()];
    state.core.config = config.into();
    let app = mcp_backbone::router(state);
    moodle.accept(&"a".repeat(32), 43);

    let (attempt_id, launch_url, preauth) = start(&app, "", "/").await;
    let pasted = app_link(&moodle.url, &launch_url, &"a".repeat(32), None);
    let refused = complete(&app, &preauth, &attempt_id, &pasted).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.json()["error"], "not_allowed");
    assert!(set_cookie(&refused, "ilmcp_session").is_none());
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(users, 0);

    // An unreachable site is iLearning's failure, not the link's.
    let app = mcp_backbone::router(testing::state_with_moodle(db, "http://127.0.0.1:9/").await);
    let (attempt_id, launch_url, preauth) = start(&app, "", "/").await;
    let pasted = app_link("http://127.0.0.1:9/", &launch_url, &"a".repeat(32), None);
    let failed = complete(&app, &preauth, &attempt_id, &pasted).await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY);
    assert_eq!(failed.json()["error"], "moodle_error");
}

#[sqlx::test(migrations = false)]
async fn me_reports_the_link(db: PgPool) {
    let state = testing::state(db).await;
    let (user_id, cookie) = testing::sign_in(&state, 7).await;
    let app = mcp_backbone::router(state.clone());
    let me = send(
        &app,
        dashboard_request(Method::GET, "/api/me", &cookie, None),
    )
    .await
    .json();
    assert_eq!(me["user"]["id"], user_id.to_string());
    assert_eq!(me["user"]["subject"], "7");
    assert_eq!(me["link"]["status"], "ok");

    LinkStore::new(&state).delete(user_id).await.unwrap();
    let me = send(
        &app,
        dashboard_request(Method::GET, "/api/me", &cookie, None),
    )
    .await
    .json();
    assert_eq!(me["link"], Value::Null);
    // The cookie keeps its legacy name.
    assert!(cookie.starts_with("ilmcp_session="), "{cookie}");
}
