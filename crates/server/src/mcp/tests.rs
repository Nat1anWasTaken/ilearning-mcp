//! The `/mcp` endpoint, driven over HTTP against a test database. The
//! configured iLearning site does not exist, so no call reaches Moodle.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    crypto::{hash_token, random_token},
    state::AppState,
    testing::{self, PUBLIC_URL, Response, dashboard_request, send},
};

const HOST: &str = "mcp.example.test";

struct Harness {
    state: AppState,
    app: Router,
    user_id: Uuid,
    cookie: String,
    session_id: Uuid,
    token: String,
}

impl Harness {
    async fn new(db: PgPool) -> Self {
        let state = testing::state(db);
        let (user_id, cookie) = testing::sign_in(&state, 7).await;
        let session_id = testing::agent_session(&state, user_id).await;
        let token = random_token();
        sqlx::query(
            "INSERT INTO oauth_access_tokens (token_hash, agent_session_id, family_id, scope,
                                              expires_at)
             VALUES ($1, $2, gen_random_uuid(), 'ilearning', now() + interval '1 hour')",
        )
        .bind(hash_token(&token))
        .bind(session_id)
        .execute(&state.db)
        .await
        .unwrap();
        Self {
            app: crate::app(state.clone()),
            state,
            user_id,
            cookie,
            session_id,
            token,
        }
    }

    async fn post_mcp(&self, body: Value) -> Response {
        let request = Request::post("/mcp")
            .header(header::HOST, HOST)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(Body::from(body.to_string()))
            .unwrap();
        send(&self.app, request).await
    }

    /// A JSON-RPC request; returns the whole response message.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response = self.post_mcp(body).await;
        assert_eq!(response.status, StatusCode::OK, "{method}");
        response.json()
    }

    /// Calls a tool; returns its `CallToolResult`.
    async fn call(&self, tool: &str, arguments: Value) -> Value {
        let response = self
            .rpc(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await;
        response["result"].clone()
    }

    async fn dashboard(&self, method: Method, uri: &str) -> Response {
        let request = dashboard_request(method, uri, &self.cookie, None);
        send(&self.app, request).await
    }

    async fn tool_calls(&self, query: &str) -> Value {
        let response = self
            .dashboard(Method::GET, &format!("/api/tool-calls{query}"))
            .await;
        assert_eq!(response.status, StatusCode::OK);
        response.json()
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn initialize_and_tools_list(db: PgPool) {
    let harness = Harness::new(db).await;
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
    assert!(info["capabilities"]["tools"].is_object());
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

    let unknown = harness
        .rpc("tools/call", json!({ "name": "nope", "arguments": {} }))
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
}

#[sqlx::test(migrations = "../../migrations")]
async fn calls_fail_cleanly_and_are_logged(db: PgPool) {
    let harness = Harness::new(db).await;
    let invalid = harness.call("get_course_contents", json!({})).await;
    assert_eq!(invalid["isError"], true);
    assert!(
        text(&invalid).starts_with("Invalid arguments: missing field `course_id`"),
        "{invalid}"
    );
    // Arguments that do not parse are logged without long strings or file bodies.
    let content = "QUJD".repeat(1000);
    harness
        .call(
            "submit_assignment",
            json!({ "files": [{ "filename": "a.bin", "base64": content }] }),
        )
        .await;
    // The site is unreachable, so a valid call fails at iLearning.
    let unreachable = harness.call("get_profile", json!({})).await;
    assert_eq!(unreachable["isError"], true);

    let page = harness.tool_calls("").await;
    let items = page["items"].as_array().unwrap();
    let statuses: Vec<_> = items
        .iter()
        .map(|call| {
            (
                call["tool"].as_str().unwrap(),
                call["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            ("get_profile", "error"),
            ("submit_assignment", "error"),
            ("get_course_contents", "error"),
        ]
    );
    assert_eq!(
        items[1]["arguments"],
        json!({ "files": [{ "filename": "a.bin", "base64": "[4000 characters]" }] })
    );
    assert_eq!(items[0]["session_id"], harness.session_id.to_string());
    assert_eq!(items[0]["session_name"], "Test Agent");
    assert!(items[0]["duration_ms"].is_number());

    let detail = harness
        .dashboard(
            Method::GET,
            &format!("/api/tool-calls/{}", items[0]["id"].as_str().unwrap()),
        )
        .await
        .json();
    assert_eq!(detail["tool"], "get_profile");
    let session = harness
        .dashboard(
            Method::GET,
            &format!("/api/sessions/{}", harness.session_id),
        )
        .await
        .json();
    assert_eq!(session["call_count"], 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn unlinked_users_are_told_to_relink(db: PgPool) {
    let harness = Harness::new(db).await;
    sqlx::query("DELETE FROM ilearning_links WHERE user_id = $1")
        .bind(harness.user_id)
        .execute(&harness.state.db)
        .await
        .unwrap();
    let result = harness.call("get_profile", json!({})).await;
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("re-link"), "{result}");
    assert!(text(&result).contains(PUBLIC_URL));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_log_pages_newest_first_and_filters(db: PgPool) {
    let harness = Harness::new(db).await;
    for text in ["one", "two", "three"] {
        // Missing the recipient, so nothing is sent.
        harness.call("send_message", json!({ "text": text })).await;
    }
    harness.call("get_module", json!({})).await;
    let first = harness.tool_calls("?limit=2&tool=send_message").await;
    let texts = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| call["arguments"]["text"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(texts(&first), ["three", "two"]);
    let before = first["next_before"].as_str().unwrap();
    let query = format!(
        "?limit=2&tool=send_message&before={}",
        before.replace('+', "%2B")
    );
    let rest = harness.tool_calls(&query).await;
    assert_eq!(texts(&rest), ["one"]);
    assert_eq!(rest["next_before"], Value::Null);

    let other = harness
        .tool_calls(&format!("?session_id={}", Uuid::new_v4()))
        .await;
    assert_eq!(other["items"], json!([]));
    assert_eq!(harness.tool_calls("?status=ok").await["items"], json!([]));
    assert_eq!(
        harness.tool_calls("?status=error&limit=50").await["items"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let bogus = harness
        .dashboard(Method::GET, "/api/tool-calls?status=bogus")
        .await;
    assert_eq!(bogus.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_endpoint_demands_a_token_and_our_host(db: PgPool) {
    let harness = Harness::new(db).await;
    let anonymous = Request::post("/mcp")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, "http://localhost:6274")
        .body(Body::empty())
        .unwrap();
    let response = send(&harness.app, anonymous).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers[header::WWW_AUTHENTICATE],
        format!(
            r#"Bearer resource_metadata="{PUBLIC_URL}/.well-known/oauth-protected-resource/mcp""#
        )
    );
    let exposed = response.headers[header::ACCESS_CONTROL_EXPOSE_HEADERS]
        .to_str()
        .unwrap();
    assert!(exposed.contains("www-authenticate"), "{exposed}");

    let preflight = Request::builder()
        .method(Method::OPTIONS)
        .uri("/mcp")
        .header(header::ORIGIN, "http://localhost:6274")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,mcp-protocol-version",
        )
        .body(Body::empty())
        .unwrap();
    let response = send(&harness.app, preflight).await;
    assert_eq!(response.status, StatusCode::OK);
    let allowed = response.headers[header::ACCESS_CONTROL_ALLOW_HEADERS]
        .to_str()
        .unwrap();
    assert!(allowed.contains("authorization") && allowed.contains("mcp-protocol-version"));

    let rebound = Request::post("/mcp")
        .header(header::HOST, "attacker.example")
        .header(header::AUTHORIZATION, format!("Bearer {}", harness.token))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ))
        .unwrap();
    assert_eq!(
        send(&harness.app, rebound).await.status,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn file_links_must_be_signed(db: PgPool) {
    let harness = Harness::new(db).await;
    let request = Request::get("/files/bogus.link")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&harness.app, request).await.status,
        StatusCode::FORBIDDEN
    );
}
