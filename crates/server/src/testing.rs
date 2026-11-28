//! Test state for iLearning on a migrated test database, and [`MockMoodle`],
//! a stand-in for the iLearning web service. The framework's `testing`
//! module has the rest.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response as AxumResponse},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use mcp_backbone::{
    App, AppState,
    auth::Profile,
    testing::{self, sign_in_as},
};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

pub use mcp_backbone::testing::{
    PUBLIC_URL, Response, access_token, agent_session, dashboard_request, mcp_request, send,
};

use crate::{
    config::{ILearningConfig, site_url},
    links,
    service::ILearningService,
};

/// The (unreachable) iLearning site tests are configured with by default.
pub const ILEARNING_URL: &str = "https://ilearning.example.test/";

pub type State_ = AppState<ILearningService>;

/// App state whose iLearning site is at `ilearning_url`.
pub async fn state_with_moodle(db: PgPool, ilearning_url: &str) -> State_ {
    testing::migrate(&db, None).await;
    let config = ILearningConfig {
        ilearning_url: site_url(ilearning_url).expect("a valid site URL"),
        token_duration_days: 84,
    };
    testing::app_state(App::new(ILearningService::new(config)), db).await
}

pub async fn state(db: PgPool) -> State_ {
    state_with_moodle(db, ILEARNING_URL).await
}

/// A signed-in dashboard user with a linked (fake) iLearning token, as
/// Moodle user `moodle_userid`; returns its ID and the `Cookie` header value.
pub async fn sign_in(state: &State_, moodle_userid: i64) -> (Uuid, String) {
    let mut profile = Profile::new(moodle_userid.to_string());
    profile.login = Some(format!("user{moodle_userid}"));
    profile.name = Some(format!("User {moodle_userid}"));
    let (user_id, cookie) = sign_in_as(state, &profile).await;
    link(state, user_id, &token(moodle_userid)).await;
    (user_id, cookie)
}

/// The fake token [`sign_in`] links for Moodle user `moodle_userid`.
pub fn token(moodle_userid: i64) -> String {
    format!("{moodle_userid:032x}")
}

/// Stores `token` as the user's iLearning token.
pub async fn link(state: &State_, user_id: Uuid, token: &str) {
    let tokens = moodle::Tokens {
        token: token.to_owned(),
        privatetoken: None,
    };
    links::store_linked(state, &state.service.config, user_id, &tokens)
        .await
        .expect("store link");
}

/// The `passport` of a launch URL.
pub fn passport(launch_url: &str) -> String {
    url::Url::parse(launch_url)
        .expect("a launch URL")
        .query_pairs()
        .find(|(key, _)| key == "passport")
        .map(|(_, value)| value.into_owned())
        .expect("a passport")
}

/// The `moodlemobile://` link iLearning shows after signing in to the site
/// at `wwwroot` for the launch URL's passport.
pub fn app_link(
    wwwroot: &str,
    launch_url: &str,
    token: &str,
    privatetoken: Option<&str>,
) -> String {
    let root = wwwroot.trim_end_matches('/');
    let signature: String = Md5::digest(format!("{root}{}", passport(launch_url)))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut payload = format!("{signature}:::{token}");
    if let Some(privatetoken) = privatetoken {
        payload.push_str(&format!(":::{privatetoken}"));
    }
    format!("moodlemobile://token={}", STANDARD.encode(payload))
}

/// An iLearning stand-in on a loopback port: the REST web service, answering
/// `core_webservice_get_site_info` for the tokens it accepts, and
/// `pluginfile.php`, serving every file as `notes.txt` saying `hello`.
pub struct MockMoodle {
    pub url: String,
    /// Accepted tokens, by the Moodle user they belong to.
    pub tokens: Arc<Mutex<HashMap<String, i64>>>,
}

impl MockMoodle {
    pub async fn start() -> Self {
        let tokens: Arc<Mutex<HashMap<String, i64>>> = Arc::default();
        let app = Router::new()
            .route("/webservice/rest/server.php", post(rest))
            .route("/webservice/pluginfile.php/{*path}", get(pluginfile))
            .with_state(tokens.clone());
        let url = testing::serve(app).await;
        Self {
            url: format!("{url}/"),
            tokens,
        }
    }

    /// Test state talking to this mock.
    pub async fn state(&self, db: PgPool) -> State_ {
        state_with_moodle(db, &self.url).await
    }

    pub fn accept(&self, token: &str, moodle_userid: i64) {
        self.tokens
            .lock()
            .unwrap()
            .insert(token.to_owned(), moodle_userid);
    }

    pub fn revoke(&self, token: &str) {
        self.tokens.lock().unwrap().remove(token);
    }
}

async fn rest(
    State(tokens): State<Arc<Mutex<HashMap<String, i64>>>>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    let userid = form
        .get("wstoken")
        .and_then(|token| tokens.lock().unwrap().get(token).copied());
    let Some(userid) = userid else {
        return invalid_token();
    };
    match form.get("wsfunction").map(String::as_str) {
        Some("core_webservice_get_site_info") => Json(json!({
            "userid": userid,
            "username": format!("s{userid}"),
            "fullname": format!("Student {userid}"),
            "userpictureurl": format!("https://ilearning.example/pic/{userid}"),
            "siteurl": "https://ilearning.example",
            "sitename": "i-learning",
            "lang": "en",
        })),
        other => Json(json!({
            "exception": "moodle_exception", "errorcode": "invalidrecord",
            "message": format!("no mock for {other:?}"),
        })),
    }
}

fn invalid_token() -> Json<Value> {
    Json(json!({
        "exception": "moodle_exception", "errorcode": "invalidtoken",
        "message": "Invalid token - token not found",
    }))
}

async fn pluginfile(
    State(tokens): State<Arc<Mutex<HashMap<String, i64>>>>,
    Query(query): Query<HashMap<String, String>>,
) -> AxumResponse {
    let accepted = query
        .get("token")
        .is_some_and(|token| tokens.lock().unwrap().contains_key(token));
    if !accepted {
        // As pluginfile.php does: JSON, with 200.
        return invalid_token().into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "text/plain"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"notes.txt\"",
            ),
        ],
        "hello",
    )
        .into_response()
}
