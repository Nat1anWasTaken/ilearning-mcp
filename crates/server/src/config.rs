//! Runtime configuration from the environment.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use url::Url;

/// Runtime configuration, read once from the environment at startup.
#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    /// Externally visible origin, without a trailing slash (e.g. `https://ilearning-mcp.nath.tw`).
    pub public_url: String,
    pub database_url: String,
    /// The iLearning (Moodle) site root, with a trailing slash.
    pub ilearning_url: Url,
    /// Moodle usernames allowed to sign in (lowercase). Empty means anyone.
    pub allowed_moodle_usernames: Vec<String>,
    /// Assumed lifetime of a freshly issued iLearning token.
    pub token_duration_days: i64,
    /// Key for encrypting iLearning tokens at rest.
    pub encryption_key: [u8; 32],
    /// Directory containing the built dashboard SPA.
    pub web_dist: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let public_url = required("PUBLIC_URL")?.trim_end_matches('/').to_owned();
        Url::parse(&public_url).context("PUBLIC_URL is not a valid URL")?;

        let encryption_key: [u8; 32] = STANDARD
            .decode(required("ENCRYPTION_KEY")?)
            .context("ENCRYPTION_KEY must be base64")?
            .try_into()
            .ok()
            .context("ENCRYPTION_KEY must decode to exactly 32 bytes")?;

        let mut ilearning_url = Url::parse(
            &optional("ILEARNING_URL").unwrap_or_else(|| "https://ilearning.cycu.edu.tw".into()),
        )
        .context("ILEARNING_URL is not a valid URL")?;
        if !ilearning_url.path().ends_with('/') {
            let path = format!("{}/", ilearning_url.path());
            ilearning_url.set_path(&path);
        }

        Ok(Self {
            bind_addr: optional("BIND_ADDR")
                .unwrap_or_else(|| "0.0.0.0:8080".into())
                .parse()
                .context("BIND_ADDR is not a socket address")?,
            public_url,
            database_url: required("DATABASE_URL")?,
            ilearning_url,
            allowed_moodle_usernames: optional("ALLOWED_MOODLE_USERNAMES")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            token_duration_days: match optional("TOKEN_DURATION_DAYS") {
                Some(v) => v
                    .trim()
                    .parse()
                    .ok()
                    .filter(|days| *days > 0)
                    .context("TOKEN_DURATION_DAYS must be a positive integer")?,
                None => 84,
            },
            encryption_key,
            web_dist: optional("WEB_DIST")
                .unwrap_or_else(|| "web/dist".into())
                .into(),
        })
    }

    /// The canonical MCP resource identifier (RFC 8707 / RFC 9728).
    pub fn mcp_resource(&self) -> String {
        format!("{}/mcp", self.public_url)
    }

    /// The host name of `public_url`; empty only for a `Config` built by hand
    /// with an invalid URL.
    pub fn public_host(&self) -> String {
        Url::parse(&self.public_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Whether `value` names the MCP resource in an RFC 8707 `resource` parameter.
    /// Tolerates a trailing slash and the bare origin, which some clients send.
    pub fn is_mcp_resource(&self, value: &str) -> bool {
        let value = value.strip_suffix('/').unwrap_or(value);
        value == self.mcp_resource() || value == self.public_url
    }

    /// Cookies get the `Secure` attribute when the dashboard is served over https.
    pub fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// Whether a browser `Origin` header names this server, compared as
    /// serialized origins so case and explicit default ports don't matter.
    pub fn is_own_origin(&self, origin: &str) -> bool {
        serialized_origin(origin).is_some_and(|o| Some(o) == serialized_origin(&self.public_url))
    }

    /// The dashboard allowlist rule. Queries that must re-check it for an
    /// existing session use [`username_allowed_sql!`], its SQL twin.
    pub fn is_username_allowed(&self, username: &str) -> bool {
        self.allowed_moodle_usernames.is_empty()
            || self
                .allowed_moodle_usernames
                .contains(&username.to_lowercase())
    }

    /// The assumed lifetime of a fresh iLearning token.
    pub fn token_duration(&self) -> chrono::TimeDelta {
        chrono::TimeDelta::days(self.token_duration_days)
    }
}

/// SQL form of [`Config::is_username_allowed`], for `concat!` into a query
/// that aliases `users` as `u` and binds `allowed_moodle_usernames` as `$2`.
macro_rules! username_allowed_sql {
    () => {
        "(cardinality($2::text[]) = 0 OR lower(u.username) = ANY($2))"
    };
}
pub(crate) use username_allowed_sql;

/// `None` for unparsable URLs and opaque origins such as `null`.
fn serialized_origin(url: &str) -> Option<String> {
    let origin = Url::parse(url).ok()?.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

fn required(key: &str) -> Result<String> {
    match optional(key) {
        Some(v) => Ok(v),
        None => bail!("missing required environment variable {key}"),
    }
}

fn optional(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::serialized_origin;

    #[test]
    fn origins_compare_normalized() {
        let ours = serialized_origin("https://ILearning-MCP.nath.tw:443");
        assert_eq!(ours.as_deref(), Some("https://ilearning-mcp.nath.tw"));
        assert_eq!(serialized_origin("https://ilearning-mcp.nath.tw"), ours);
        assert_ne!(serialized_origin("http://ilearning-mcp.nath.tw"), ours);
        assert_ne!(
            serialized_origin("https://ilearning-mcp.nath.tw:8443"),
            ours
        );
        assert_eq!(serialized_origin("null"), None);
    }
}
