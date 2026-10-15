//! OAuth clients, registered dynamically (RFC 7591) or described by a Client
//! ID Metadata Document (see [`super::cimd`]).

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use url::Url;

use super::{OAuthError, cimd, redirect_uri};

pub const SUPPORTED_GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// How long a fetched metadata document is trusted before it is fetched again.
const CIMD_MAX_AGE: TimeDelta = TimeDelta::hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AuthMethod {
    None,
    ClientSecretPost,
    ClientSecretBasic,
    /// A signed JWT assertion (RFC 7523); metadata-document clients only.
    PrivateKeyJwt,
}

impl AuthMethod {
    pub const ALL: [Self; 4] = [
        Self::None,
        Self::ClientSecretPost,
        Self::ClientSecretBasic,
        Self::PrivateKeyJwt,
    ];
}

#[derive(Debug, Clone, Copy, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum Source {
    /// Dynamic client registration (RFC 7591).
    Dcr,
    /// Client ID Metadata Document.
    Cimd,
}

/// The columns of [`Client`], for `concat!` into its queries.
macro_rules! client_columns {
    () => {
        "client_id, client_secret_hash, redirect_uris, token_endpoint_auth_method, \
         metadata->>'jwks_uri' AS jwks_uri, updated_at"
    };
}

#[derive(Debug, FromRow)]
pub struct Client {
    pub client_id: String,
    pub client_secret_hash: Option<Vec<u8>>,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: AuthMethod,
    /// Where a `private_key_jwt` client publishes its keys.
    pub jwks_uri: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Validated client metadata, ready to store.
#[derive(Debug)]
pub struct Registration {
    pub client_name: Option<String>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    pub redirect_uris: Vec<String>,
    /// The supported subset of what the client asked for.
    pub grant_types: Vec<String>,
    pub auth_method: AuthMethod,
    /// The metadata as submitted.
    pub metadata: Value,
}

/// The RFC 7591 fields this server acts on; others are kept only in `metadata`.
#[derive(Deserialize)]
struct ClientMetadata {
    #[serde(default)]
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<AuthMethod>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    client_name: Option<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
}

impl Registration {
    /// Validates client metadata. `default_auth_method` applies when the
    /// metadata names none (RFC 7591 says `client_secret_basic`).
    ///
    /// Unsupported grant types are dropped rather than rejected: Claude, for
    /// one, also asks for `urn:ietf:params:oauth:grant-type:jwt-bearer`.
    pub fn from_metadata(
        metadata: Value,
        default_auth_method: AuthMethod,
    ) -> Result<Self, OAuthError> {
        let fields: ClientMetadata = serde_json::from_value(metadata.clone())
            .map_err(|err| OAuthError::invalid_client_metadata(err.to_string()))?;

        if fields.redirect_uris.is_empty() {
            return Err(OAuthError::invalid_redirect_uri(
                "redirect_uris is required",
            ));
        }
        for uri in &fields.redirect_uris {
            redirect_uri::validate(uri)
                .map_err(|reason| OAuthError::invalid_redirect_uri(format!("{uri}: {reason}")))?;
        }

        let requested = fields
            .grant_types
            .unwrap_or_else(|| vec!["authorization_code".to_owned()]);
        if !requested.iter().any(|g| g == "authorization_code") {
            return Err(OAuthError::invalid_client_metadata(
                "grant_types must include authorization_code",
            ));
        }
        let grant_types = SUPPORTED_GRANT_TYPES
            .into_iter()
            .filter(|supported| requested.iter().any(|g| g == supported))
            .map(str::to_owned)
            .collect();

        if fields
            .response_types
            .is_some_and(|types| !types.iter().any(|t| t == "code"))
        {
            return Err(OAuthError::invalid_client_metadata(
                "response_types must include code",
            ));
        }

        Ok(Self {
            client_name: fields
                .client_name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            client_uri: fields.client_uri.filter(|uri| is_web_url(uri)),
            logo_uri: fields.logo_uri.filter(|uri| is_web_url(uri)),
            redirect_uris: fields.redirect_uris,
            grant_types,
            auth_method: fields
                .token_endpoint_auth_method
                .unwrap_or(default_auth_method),
            metadata,
        })
    }
}

/// Informational URLs are shown on the consent screen, so only web URLs are kept.
fn is_web_url(uri: &str) -> bool {
    Url::parse(uri).is_ok_and(|url| matches!(url.scheme(), "https" | "http"))
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("unknown client_id")]
    Unknown,
    #[error("client metadata document rejected: {0:#}")]
    Document(anyhow::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn find(db: &PgPool, client_id: &str) -> sqlx::Result<Option<Client>> {
    sqlx::query_as(concat!(
        "SELECT ",
        client_columns!(),
        " FROM oauth_clients WHERE client_id = $1"
    ))
    .bind(client_id)
    .fetch_optional(db)
    .await
}

/// Looks up a client for an authorization request, fetching (or refreshing)
/// its metadata document when the client ID is a CIMD URL. `document_url` is
/// [`cimd::document_url`] of `client_id`, which the caller already needed.
pub async fn resolve(
    db: &PgPool,
    client_id: &str,
    document_url: Option<&Url>,
) -> Result<Client, ResolveError> {
    let known = find(db, client_id).await?;
    let Some(url) = document_url else {
        return known.ok_or(ResolveError::Unknown);
    };
    let stale = match known {
        Some(client) if Utc::now() - client.updated_at < CIMD_MAX_AGE => return Ok(client),
        known => known,
    };
    match cimd::fetch(url).await {
        Ok(registration) => Ok(save(db, client_id, None, Source::Cimd, &registration).await?),
        // A stale registration beats none while the document host is down.
        Err(err) => {
            let Some(client) = stale else {
                return Err(ResolveError::Document(err));
            };
            tracing::warn!(client_id, error = %err, "client metadata refresh failed; using cached copy");
            Ok(client)
        }
    }
}

/// Stores a client. Saving a metadata-document client again refreshes it in
/// place; registered clients get fresh IDs, so they never conflict.
pub async fn save(
    db: &PgPool,
    client_id: &str,
    secret_hash: Option<&[u8]>,
    source: Source,
    registration: &Registration,
) -> sqlx::Result<Client> {
    sqlx::query_as(concat!(
        "INSERT INTO oauth_clients (client_id, client_secret_hash, client_name, client_uri,
             logo_uri, redirect_uris, grant_types, token_endpoint_auth_method, source, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (client_id) DO UPDATE SET client_name = EXCLUDED.client_name,
             client_uri = EXCLUDED.client_uri, logo_uri = EXCLUDED.logo_uri,
             redirect_uris = EXCLUDED.redirect_uris, grant_types = EXCLUDED.grant_types,
             token_endpoint_auth_method = EXCLUDED.token_endpoint_auth_method,
             metadata = EXCLUDED.metadata, updated_at = now()
         WHERE EXCLUDED.source = 'cimd'
         RETURNING ",
        client_columns!()
    ))
    .bind(client_id)
    .bind(secret_hash)
    .bind(&registration.client_name)
    .bind(&registration.client_uri)
    .bind(&registration.logo_uri)
    .bind(&registration.redirect_uris)
    .bind(&registration.grant_types)
    .bind(registration.auth_method)
    .bind(source)
    .bind(&registration.metadata)
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn register(metadata: Value) -> Result<Registration, OAuthError> {
        Registration::from_metadata(metadata, AuthMethod::ClientSecretBasic)
    }

    #[test]
    fn accepts_claude_registration_and_drops_jwt_bearer() {
        let registration = register(json!({
            "client_name": "Claude",
            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
            "grant_types": [
                "authorization_code",
                "refresh_token",
                "urn:ietf:params:oauth:grant-type:jwt-bearer"
            ],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": "ilearning"
        }))
        .unwrap();
        assert_eq!(
            registration.grant_types,
            ["authorization_code", "refresh_token"]
        );
        assert_eq!(registration.auth_method, AuthMethod::None);
        assert_eq!(registration.client_name.as_deref(), Some("Claude"));
        assert_eq!(registration.metadata["scope"], "ilearning");
    }

    #[test]
    fn applies_rfc_7591_defaults() {
        let registration =
            register(json!({"redirect_uris": ["http://127.0.0.1:1234/cb"]})).unwrap();
        assert_eq!(registration.grant_types, ["authorization_code"]);
        assert_eq!(registration.auth_method, AuthMethod::ClientSecretBasic);
    }

    #[test]
    fn rejects_invalid_metadata() {
        for metadata in [
            json!({}),
            json!({"redirect_uris": []}),
            json!({"redirect_uris": "https://a.example/cb"}),
            json!({"redirect_uris": ["http://evil.example/cb"]}),
            json!({"redirect_uris": ["javascript:alert(1)"]}),
            json!({"redirect_uris": ["https://a.example/cb"], "grant_types": ["refresh_token"]}),
            json!({"redirect_uris": ["https://a.example/cb"], "grant_types": ["client_credentials"]}),
            json!({"redirect_uris": ["https://a.example/cb"], "response_types": ["token"]}),
            json!({"redirect_uris": ["https://a.example/cb"], "token_endpoint_auth_method": "tls_client_auth"}),
        ] {
            assert!(register(metadata.clone()).is_err(), "{metadata}");
        }
    }

    #[test]
    fn drops_non_web_informational_urls() {
        let registration = register(json!({
            "redirect_uris": ["https://a.example/cb"],
            "client_uri": "javascript:alert(1)",
            "logo_uri": "https://a.example/logo.png",
            "client_name": "  "
        }))
        .unwrap();
        assert_eq!(registration.client_uri, None);
        assert_eq!(
            registration.logo_uri.as_deref(),
            Some("https://a.example/logo.png")
        );
        assert_eq!(registration.client_name, None);
    }
}
