//! Moodle layer for CYCU i-learning 2.0.
//!
//! Talks to Moodle's mobile web service on behalf of one user at a time,
//! parses the mobile-app launch links used to sign in, renews tokens through
//! a short-lived web session, and exposes high-level operations that return
//! compact, agent-friendly JSON. Knows nothing about HTTP serving, OAuth,
//! MCP or the database.
//!
//! - [`Moodle`] is the shared entry point for one site; [`Moodle::account`]
//!   binds it to a web service token.
//! - [`Account`] runs REST calls ([`Account::call`]), file transfers
//!   ([`Account::download`], [`Account::upload_draft`],
//!   [`Account::read_file`]) and one method per agent tool, taking the
//!   request types in [`types`].
//! - [`link`] builds launch URLs and verifies pasted `moodlemobile://` links;
//!   [`renew`] mints a fresh token from a private token.

mod error;
mod files;
mod ops;
mod rest;
mod text;

pub mod link;
pub mod renew;
pub mod types;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use url::Url;

pub use error::{Error, Result};
pub use files::{
    Download, FileChunk, FileContent, MAX_CHUNK_BYTES, MAX_IMAGE_BYTES, UploadFile,
    file_name_and_mime,
};
pub use link::Tokens;

/// User agent for every request. Moodle only issues autologin keys to
/// clients whose user agent contains `MoodleMobile`.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; ilearning-mcp/",
    env!("CARGO_PKG_VERSION"),
    ") MoodleMobile"
);

/// One Moodle site. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Moodle {
    http: reqwest::Client,
    /// Same as `http`, but never follows redirects (web session requests).
    no_redirect: reqwest::Client,
    wwwroot: Url,
}

impl Moodle {
    /// `wwwroot` is the site root, e.g. `https://ilearning.cycu.edu.tw`.
    pub fn new(wwwroot: Url) -> Self {
        let builder = || {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(180))
        };
        Self {
            http: builder().build().expect("reqwest client"),
            no_redirect: builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client"),
            wwwroot,
        }
    }

    pub fn wwwroot(&self) -> &Url {
        &self.wwwroot
    }

    /// Operations as the owner of a web service `token`.
    pub fn account(&self, token: String) -> Account {
        Account {
            moodle: self.clone(),
            token: token.into(),
            me: Arc::default(),
        }
    }

    /// `wwwroot` without a trailing slash, as Moodle's `$CFG->wwwroot`.
    pub(crate) fn root(&self) -> &str {
        self.wwwroot.as_str().trim_end_matches('/')
    }

    /// Absolute URL of a site path such as `/webservice/upload.php`.
    pub(crate) fn endpoint(&self, path: &str) -> Url {
        Url::parse(&format!("{}{path}", self.root())).expect("valid endpoint URL")
    }
}

/// Operations as one Moodle user. Cheap to clone; clones share the cached
/// [`SiteInfo`].
#[derive(Clone)]
pub struct Account {
    moodle: Moodle,
    token: Arc<str>,
    me: Arc<OnceCell<SiteInfo>>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("wwwroot", &self.moodle.root())
            .finish_non_exhaustive()
    }
}

/// The token owner, from `core_webservice_get_site_info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteInfo {
    pub userid: i64,
    pub username: String,
    pub fullname: String,
    pub userpictureurl: Option<String>,
    pub siteurl: String,
    /// Site name, with multi-language tags resolved.
    pub sitename: String,
    /// The user's language, e.g. `zh_tw`.
    pub lang: String,
}

impl Account {
    pub fn moodle(&self) -> &Moodle {
        &self.moodle
    }

    /// Fetches the token owner's identity (and caches it on this handle).
    /// Also the cheapest way to check that a token works.
    pub async fn site_info(&self) -> Result<SiteInfo> {
        let raw = self
            .call("core_webservice_get_site_info", serde_json::json!({}))
            .await?;
        let text = |key: &str| raw.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        let lang = text("lang").to_owned();
        let info = SiteInfo {
            userid: raw
                .get("userid")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| Error::decode("site info has no userid"))?,
            username: text("username").to_owned(),
            fullname: text::mlang(text("fullname"), &lang),
            userpictureurl: Some(text("userpictureurl").to_owned()).filter(|s| !s.is_empty()),
            siteurl: text("siteurl").to_owned(),
            sitename: text::mlang(text("sitename"), &lang),
            lang,
        };
        let _ = self.me.set(info.clone());
        Ok(info)
    }

    /// The cached [`SiteInfo`], fetched on first use.
    pub(crate) async fn me(&self) -> Result<&SiteInfo> {
        if let Some(info) = self.me.get() {
            return Ok(info);
        }
        let info = self.site_info().await?;
        Ok(self.me.get_or_init(|| async { info }).await)
    }
}
