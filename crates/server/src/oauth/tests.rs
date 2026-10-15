//! End-to-end authorization flows over HTTP against a test database.

use axum::{
    Extension, Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    middleware,
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sqlx::PgPool;
use url::{Url, form_urlencoded};

use super::{AgentPrincipal, require_agent};
use crate::{
    state::AppState,
    testing::{self, PUBLIC_URL, Response, send},
};

const REDIRECT_URI: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "dBjftJeZ4CVP-mJ92K9-OInbo0-2G2SVvBNLgSsxNjk";
const CHALLENGE: &str = "vkRMquJVBhilqWKo7Eh-mZY-FzEJuYL95XVg6gH_Qpk";

fn claude_metadata() -> Value {
    json!({
        "client_name": "Claude",
        "redirect_uris": [REDIRECT_URI],
        "grant_types": [
            "authorization_code",
            "refresh_token",
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        ],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none"
    })
}

/// An authorization request URL; `overrides` replace or add parameters.
fn authorize_url(client_id: &str, redirect_uri: &str, overrides: &[(&str, &str)]) -> String {
    let defaults = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code_challenge", CHALLENGE),
        ("code_challenge_method", "S256"),
        ("state", "xyz"),
    ];
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            defaults
                .iter()
                .filter(|(key, _)| overrides.iter().all(|(k, _)| k != key)),
        )
        .extend_pairs(overrides)
        .finish();
    format!("/oauth/authorize?{query}")
}

fn query_param(url: &str, name: &str) -> Option<String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

struct Harness {
    app: Router,
    state: AppState,
    cookie: String,
}

impl Harness {
    async fn new(db: PgPool) -> Self {
        let state = testing::state(db);
        let (_, cookie) = testing::sign_in(&state, 1).await;
        Self {
            app: crate::app(state.clone()),
            state,
            cookie,
        }
    }

    async fn register(&self, metadata: Value) -> Value {
        let response = send(
            &self.app,
            Request::post("/oauth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(metadata.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(
            response.status,
            StatusCode::CREATED,
            "{:?}",
            response.json()
        );
        response.json()
    }

    async fn browse(&self, uri: &str, signed_in: bool) -> Response {
        let mut request = Request::get(uri);
        if signed_in {
            request = request.header(header::COOKIE, &self.cookie);
        }
        send(&self.app, request.body(Body::empty()).unwrap()).await
    }

    async fn api(&self, method: Method, uri: &str, body: Option<Value>) -> Response {
        let request = testing::dashboard_request(method, uri, &self.cookie, body.as_ref());
        send(&self.app, request).await
    }

    async fn token(&self, form: &[(&str, &str)], basic: Option<(&str, &str)>) -> Response {
        let mut request = Request::post("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some((id, secret)) = basic {
            let credentials = STANDARD.encode(format!("{id}:{secret}"));
            request = request.header(header::AUTHORIZATION, format!("Basic {credentials}"));
        }
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        send(&self.app, request.body(Body::from(body)).unwrap()).await
    }

    /// Authorizes and consents; returns the authorization response redirect.
    /// Starts an authorization as the signed-in user; returns the request ID.
    async fn authorize(&self, client_id: &str) -> String {
        let response = self
            .browse(&authorize_url(client_id, REDIRECT_URI, &[]), true)
            .await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
        response
            .location()
            .strip_prefix("/consent/")
            .unwrap()
            .to_owned()
    }

    /// The consent page's view of a fresh authorization request.
    async fn pending_request(&self, client_id: &str) -> Value {
        let id = self.authorize(client_id).await;
        let uri = format!("/api/oauth/requests/{id}");
        self.api(Method::GET, &uri, None).await.json()
    }

    async fn consent(&self, client_id: &str, decision: Value) -> String {
        let id = self.authorize(client_id).await;
        let response = self
            .api(
                Method::POST,
                &format!("/api/oauth/requests/{id}"),
                Some(decision),
            )
            .await;
        assert_eq!(response.status, StatusCode::OK);
        response.json()["redirect_to"].as_str().unwrap().to_owned()
    }

    async fn code(&self, client_id: &str) -> String {
        let redirect = self.consent(client_id, json!({ "approve": true })).await;
        query_param(&redirect, "code").unwrap()
    }

    async fn exchange(&self, client_id: &str, code: &str) -> Response {
        self.token(
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
                ("code_verifier", VERIFIER),
                ("client_id", client_id),
            ],
            None,
        )
        .await
    }

    /// Registers a public client and runs the whole flow; returns the token response.
    async fn connect(&self) -> (String, Value) {
        let client_id = self.register(claude_metadata()).await["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let code = self.code(&client_id).await;
        let response = self.exchange(&client_id, &code).await;
        assert_eq!(response.status, StatusCode::OK);
        (client_id, response.json())
    }

    async fn token_owned(&self, form: &[(&str, String)]) -> Response {
        let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.token(&form, None).await
    }

    async fn refresh(&self, client_id: &str, refresh_token: &str) -> Response {
        self.token(
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", client_id),
            ],
            None,
        )
        .await
    }

    /// Calls a route behind [`require_agent`] that echoes the principal.
    async fn protected(&self, authorization: Option<&str>) -> Response {
        let app = Router::new()
            .route(
                "/protected",
                get(|Extension(agent): Extension<AgentPrincipal>| async move {
                    agent.session_id.to_string()
                }),
            )
            .layer(middleware::from_fn_with_state(
                self.state.clone(),
                require_agent,
            ))
            .with_state(self.state.clone());
        let mut request = Request::get("/protected");
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        send(&app, request.body(Body::empty()).unwrap()).await
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn authorization_code_flow(db: PgPool) {
    let h = Harness::new(db).await;
    let client = h.register(claude_metadata()).await;
    assert!(client.get("client_secret").is_none());
    assert_eq!(
        client["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(client["token_endpoint_auth_method"], "none");
    let client_id = client["client_id"].as_str().unwrap();

    let url = authorize_url(
        client_id,
        REDIRECT_URI,
        &[("resource", "https://mcp.example.test/mcp")],
    );
    let response = h.browse(&url, false).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let next: String = form_urlencoded::byte_serialize(url.as_bytes()).collect();
    assert_eq!(response.location(), format!("/login?next={next}"));

    let response = h.browse(&url, true).await;
    let id = response
        .location()
        .strip_prefix("/consent/")
        .unwrap()
        .to_owned();
    let request_uri = format!("/api/oauth/requests/{id}");
    let request = h.api(Method::GET, &request_uri, None).await.json();
    assert_eq!(request["client_name"], "Claude");
    assert_eq!(request["origin"], "claude.ai");
    assert_eq!(request["redirect_uri"], REDIRECT_URI);
    assert_eq!(request["scope"], "ilearning");

    let redirect = h
        .api(Method::POST, &request_uri, Some(json!({ "approve": true })))
        .await
        .json()["redirect_to"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(redirect.starts_with(REDIRECT_URI));
    assert_eq!(query_param(&redirect, "state").as_deref(), Some("xyz"));
    assert_eq!(query_param(&redirect, "iss").as_deref(), Some(PUBLIC_URL));
    let code = query_param(&redirect, "code").unwrap();
    assert_eq!(
        h.api(Method::GET, &request_uri, None).await.status,
        StatusCode::NOT_FOUND
    );

    let response = h.exchange(client_id, &code).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    let tokens = response.json();
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["scope"], "ilearning");

    let replay = h.exchange(client_id, &code).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.json()["error"], "invalid_grant");

    let sessions = h.api(Method::GET, "/api/sessions", None).await.json();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    let session = &sessions[0];
    assert_eq!(session["name"], "Claude");
    assert_eq!(session["origin"], "claude.ai");
    assert_eq!(session["call_count"], 0);

    let access = format!("Bearer {}", tokens["access_token"].as_str().unwrap());
    let response = h.protected(Some(&access)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        String::from_utf8(response.body).unwrap(),
        session["id"].as_str().unwrap()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_rotation_and_reuse_detection(db: PgPool) {
    let h = Harness::new(db).await;
    let (client_id, tokens) = h.connect().await;
    let first = tokens["refresh_token"].as_str().unwrap();

    let rotated = h.refresh(&client_id, first).await;
    assert_eq!(rotated.status, StatusCode::OK);
    let rotated = rotated.json();
    let second = rotated["refresh_token"].as_str().unwrap();
    assert_ne!(first, second);
    let access = format!("Bearer {}", rotated["access_token"].as_str().unwrap());
    assert_eq!(h.protected(Some(&access)).await.status, StatusCode::OK);

    // Replaying the rotated token revokes the family, including its successor.
    let replay = h.refresh(&client_id, first).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.json()["error"], "invalid_grant");
    assert_eq!(
        h.refresh(&client_id, second).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.protected(Some(&access)).await.status,
        StatusCode::UNAUTHORIZED,
        "access tokens of a revoked family must stop working"
    );

    // Another client cannot use someone else's refresh token.
    let (other_client, other_tokens) = h.connect().await;
    let other_refresh = other_tokens["refresh_token"].as_str().unwrap();
    assert_eq!(
        h.refresh(&client_id, other_refresh).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.refresh(&other_client, other_refresh).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_exchanges_burn_the_code(db: PgPool) {
    let h = Harness::new(db).await;
    let client_id = h.register(claude_metadata()).await["client_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let code = h.code(&client_id).await;
    let wrong_verifier = "A".repeat(43);
    let response = h
        .token(
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", REDIRECT_URI),
                ("code_verifier", &wrong_verifier),
                ("client_id", &client_id),
            ],
            None,
        )
        .await;
    assert_eq!(response.json()["error"], "invalid_grant");
    assert_eq!(
        h.exchange(&client_id, &code).await.status,
        StatusCode::BAD_REQUEST
    );

    let code = h.code(&client_id).await;
    let response = h
        .token(
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "https://claude.ai/other"),
                ("code_verifier", VERIFIER),
                ("client_id", &client_id),
            ],
            None,
        )
        .await;
    assert_eq!(response.json()["error"], "invalid_grant");

    let response = h
        .token(
            &[("grant_type", "password"), ("client_id", &client_id)],
            None,
        )
        .await;
    assert_eq!(response.json()["error"], "unsupported_grant_type");
    let response = h.token(&[("grant_type", "authorization_code")], None).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.json()["error"], "invalid_client");
}

#[sqlx::test(migrations = "../../migrations")]
async fn authorize_validates_before_trusting_the_redirect_uri(db: PgPool) {
    let h = Harness::new(db).await;
    let client_id = h.register(claude_metadata()).await["client_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let unknown = h
        .browse(&authorize_url("nope", REDIRECT_URI, &[]), true)
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    let unregistered = h
        .browse(
            &authorize_url(&client_id, "https://evil.example/cb", &[]),
            true,
        )
        .await;
    assert_eq!(unregistered.status, StatusCode::BAD_REQUEST);

    for (overrides, error) in [
        (("code_challenge_method", "plain"), "invalid_request"),
        (("code_challenge", "too-short"), "invalid_request"),
        (("resource", "https://other.example/mcp"), "invalid_target"),
        (("response_type", "token"), "unsupported_response_type"),
    ] {
        let response = h
            .browse(&authorize_url(&client_id, REDIRECT_URI, &[overrides]), true)
            .await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{overrides:?}");
        let location = response.location();
        assert!(location.starts_with(REDIRECT_URI));
        assert_eq!(
            query_param(location, "error").as_deref(),
            Some(error),
            "{overrides:?}"
        );
        assert_eq!(query_param(location, "state").as_deref(), Some("xyz"));
        assert_eq!(query_param(location, "iss").as_deref(), Some(PUBLIC_URL));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn loopback_redirects_may_use_any_port(db: PgPool) {
    let h = Harness::new(db).await;
    let client_id = h
        .register(json!({
            "client_name": "Claude Code",
            "redirect_uris": ["http://localhost/callback"],
            "token_endpoint_auth_method": "none"
        }))
        .await["client_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = authorize_url(&client_id, "http://localhost:54321/callback", &[]);
    let response = h.browse(&url, true).await;
    assert!(response.location().starts_with("/consent/"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn denial_and_reauthorization(db: PgPool) {
    let h = Harness::new(db).await;
    let client_id = h.register(claude_metadata()).await["client_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let denied = h.consent(&client_id, json!({ "approve": false })).await;
    assert_eq!(
        query_param(&denied, "error").as_deref(),
        Some("access_denied")
    );
    assert_eq!(query_param(&denied, "code"), None);
    assert_eq!(
        h.api(Method::GET, "/api/sessions", None).await.json(),
        json!([])
    );

    assert_eq!(
        h.pending_request(&client_id).await["existing_session"],
        Value::Null
    );
    h.consent(&client_id, json!({ "approve": true })).await;
    h.consent(
        &client_id,
        json!({ "approve": true, "name": " Work Claude " }),
    )
    .await;
    assert_eq!(
        h.pending_request(&client_id).await["existing_session"],
        json!({ "name": "Work Claude" })
    );
    let sessions = h.api(Method::GET, "/api/sessions", None).await.json();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["name"], "Work Claude");
}

#[sqlx::test(migrations = "../../migrations")]
async fn bearer_middleware_and_session_revocation(db: PgPool) {
    let h = Harness::new(db).await;
    let metadata_url = format!("{PUBLIC_URL}/.well-known/oauth-protected-resource/mcp");

    let missing = h.protected(None).await;
    assert_eq!(missing.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.headers[header::WWW_AUTHENTICATE],
        format!(r#"Bearer resource_metadata="{metadata_url}""#).as_str()
    );
    let invalid = h.protected(Some("Bearer not-a-token")).await;
    assert_eq!(
        invalid.headers[header::WWW_AUTHENTICATE],
        format!(r#"Bearer resource_metadata="{metadata_url}", error="invalid_token""#).as_str()
    );

    let (client_id, tokens) = h.connect().await;
    let access = format!("Bearer {}", tokens["access_token"].as_str().unwrap());
    assert_eq!(h.protected(Some(&access)).await.status, StatusCode::OK);
    let session = &h.api(Method::GET, "/api/sessions", None).await.json()[0];
    assert!(session["last_used_at"].is_string());
    let session_uri = format!("/api/sessions/{}", session["id"].as_str().unwrap());

    // Renamed sessions keep their tokens.
    let patched = h
        .api(
            Method::PATCH,
            &session_uri,
            Some(json!({ "name": "Renamed" })),
        )
        .await;
    assert_eq!(patched.json()["name"], "Renamed");
    assert_eq!(h.protected(Some(&access)).await.status, StatusCode::OK);

    assert_eq!(
        h.api(Method::DELETE, &session_uri, None).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        h.protected(Some(&access)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let refresh_token = tokens["refresh_token"].as_str().unwrap();
    assert_eq!(
        h.refresh(&client_id, refresh_token).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.api(Method::GET, &session_uri, None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn confidential_clients_must_authenticate(db: PgPool) {
    let h = Harness::new(db).await;
    for method in ["client_secret_post", "client_secret_basic"] {
        let mut metadata = claude_metadata();
        metadata["token_endpoint_auth_method"] = json!(method);
        let client = h.register(metadata).await;
        assert_eq!(client["client_secret_expires_at"], 0);
        let client_id = client["client_id"].as_str().unwrap();
        let secret = client["client_secret"].as_str().unwrap();

        let code = h.code(client_id).await;
        let grant = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", VERIFIER),
        ];
        let with_secret = |secret| {
            let mut form = grant.clone();
            form.extend([("client_id", client_id), ("client_secret", secret)]);
            form
        };
        let (posted, basic) = (with_secret(secret), Some((client_id, secret)));

        // Failed client authentication leaves the code usable.
        for (form, basic) in [
            (with_secret("wrong"), None),
            (grant.clone(), Some((client_id, "wrong"))),
            (grant.clone(), None),
        ] {
            assert_eq!(h.token(&form, basic).await.status, StatusCode::UNAUTHORIZED);
        }
        let (wrong_method, right_method) = if method == "client_secret_post" {
            ((&grant, basic), (&posted, None))
        } else {
            ((&posted, None), (&grant, basic))
        };
        let response = h.token(wrong_method.0, wrong_method.1).await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{method}");
        let response = h.token(right_method.0, right_method.1).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{method}: {:?}",
            response.json()
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn revocation_and_metadata(db: PgPool) {
    let h = Harness::new(db).await;
    let (client_id, tokens) = h.connect().await;
    let revoke = |token: String| {
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([("token", token.as_str()), ("client_id", client_id.as_str())])
            .finish();
        Request::post("/oauth/revoke")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    };
    let access = tokens["access_token"].as_str().unwrap();
    let refresh = tokens["refresh_token"].as_str().unwrap();
    assert_eq!(
        send(&h.app, revoke(access.to_owned())).await.status,
        StatusCode::OK
    );
    assert_eq!(
        h.protected(Some(&format!("Bearer {access}"))).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&h.app, revoke(refresh.to_owned())).await.status,
        StatusCode::OK
    );
    assert_eq!(
        h.refresh(&client_id, refresh).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&h.app, revoke("unknown".to_owned())).await.status,
        StatusCode::OK
    );

    let resource = h
        .browse("/.well-known/oauth-protected-resource/mcp", false)
        .await
        .json();
    assert_eq!(resource["resource"], format!("{PUBLIC_URL}/mcp"));
    assert_eq!(resource["authorization_servers"], json!([PUBLIC_URL]));
    let server = h
        .browse("/.well-known/oauth-authorization-server", false)
        .await;
    assert_eq!(server.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    let server = server.json();
    assert_eq!(server["issuer"], PUBLIC_URL);
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(server["client_id_metadata_document_supported"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn removing_a_user_from_the_allowlist_revokes_access(db: PgPool) {
    let mut h = Harness::new(db).await;
    let (_, tokens) = h.connect().await;
    let access = format!("Bearer {}", tokens["access_token"].as_str().unwrap());
    assert_eq!(h.protected(Some(&access)).await.status, StatusCode::OK);

    let mut config = (*h.state.config).clone();
    config.allowed_moodle_usernames = vec!["someone-else".to_owned()];
    h.state.config = std::sync::Arc::new(config);
    h.app = crate::app(h.state.clone());

    assert_eq!(
        h.protected(Some(&access)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.api(Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn anonymous_metadata_document_clients_sign_in_before_any_fetch(db: PgPool) {
    let h = Harness::new(db).await;
    let client_id = "https://client.example/oauth/metadata.json";
    let response = h
        .browse(&authorize_url(client_id, REDIRECT_URI, &[]), false)
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(
        response
            .location()
            .starts_with("/login?next=%2Foauth%2Fauthorize")
    );
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_clients")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(
        stored, 0,
        "no client is fetched or stored for anonymous requests"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn dynamic_registration_refuses_private_key_jwt(db: PgPool) {
    let h = Harness::new(db).await;
    let mut metadata = claude_metadata();
    metadata["token_endpoint_auth_method"] = json!("private_key_jwt");
    metadata["jwks_uri"] = json!("https://claude.ai/jwks.json");
    let response = send(
        &h.app,
        Request::post("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(metadata.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.json()["error"], "invalid_client_metadata");
}

/// A metadata-document client that authenticates with `private_key_jwt`, the
/// way ChatGPT does.
#[sqlx::test(migrations = "../../migrations")]
async fn private_key_jwt_clients_authenticate_with_signed_assertions(db: PgPool) {
    use super::client_assertion::{
        ASSERTION_TYPE,
        test_client::{CLIENT_ID, JWKS_URI, claims, sign},
    };

    let h = Harness::new(db).await;
    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, grant_types,
             token_endpoint_auth_method, source, metadata)
         VALUES ($1, 'ChatGPT', ARRAY[$2], ARRAY['authorization_code', 'refresh_token'],
             'private_key_jwt', 'cimd', jsonb_build_object('jwks_uri', $3::text))",
    )
    .bind(CLIENT_ID)
    .bind(REDIRECT_URI)
    .bind(JWKS_URI)
    .execute(&h.state.db)
    .await
    .unwrap();
    h.state.client_assertions.preload_test_keys();

    let token_endpoint = format!("{PUBLIC_URL}/oauth/token");
    let code = h.code(CLIENT_ID).await;
    let exchange = |assertion: Option<String>| {
        let mut form = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.clone()),
            ("redirect_uri", REDIRECT_URI.to_owned()),
            ("code_verifier", VERIFIER.to_owned()),
        ];
        if let Some(assertion) = assertion {
            form.push(("client_assertion_type", ASSERTION_TYPE.to_owned()));
            form.push(("client_assertion", assertion));
        }
        form
    };

    // Without an assertion, or with one for the wrong audience, the client is
    // refused and the code stays usable.
    assert_eq!(
        h.token_owned(&exchange(None)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let elsewhere = sign(&claims("https://elsewhere.example/oauth/token", "a"));
    assert_eq!(
        h.token_owned(&exchange(Some(elsewhere))).await.status,
        StatusCode::UNAUTHORIZED
    );

    let assertion = sign(&claims(&token_endpoint, "b"));
    let response = h.token_owned(&exchange(Some(assertion.clone()))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json());
    let refresh_token = response.json()["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();

    // A replayed assertion is refused; a fresh one refreshes.
    let refresh = |assertion: String| {
        vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh_token.clone()),
            ("client_assertion_type", ASSERTION_TYPE.to_owned()),
            ("client_assertion", assertion),
        ]
    };
    assert_eq!(
        h.token_owned(&refresh(assertion)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let response = h
        .token_owned(&refresh(sign(&claims(&token_endpoint, "c"))))
        .await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json());

    // A public client may not present an assertion instead of nothing.
    let (public_client, _) = h.connect().await;
    let mut form = refresh(sign(&claims(&token_endpoint, "d")));
    form.push(("client_id", public_client));
    assert_eq!(h.token_owned(&form).await.status, StatusCode::UNAUTHORIZED);
}
