//! Client ID Metadata Documents: a client whose `client_id` is an `https` URL
//! publishes its metadata as JSON at that URL instead of registering.

use std::time::Duration;

use anyhow::{Context, bail, ensure};
use serde_json::Value;
use url::Url;

use super::{
    client::{AuthMethod, Registration},
    client_assertion::SIGNING_ALGORITHMS,
};
use crate::net;

const MAX_BYTES: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

/// The document URL if `client_id` is a metadata document URL: `https`, with a
/// path, no credentials or fragment, and already in canonical form (which
/// rules out dot segments).
pub fn document_url(client_id: &str) -> Option<Url> {
    let url = Url::parse(client_id).ok()?;
    let valid = url.scheme() == "https"
        && url.path() != "/"
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.as_str() == client_id;
    valid.then_some(url)
}

/// Fetches and validates the document. The client is either public (`none`)
/// or authenticates with `private_key_jwt` using keys published on its own host.
pub async fn fetch(url: &Url) -> anyhow::Result<Registration> {
    let fetched = net::fetch_public_url(url, MAX_BYTES, TIMEOUT).await?;
    let document: Value =
        serde_json::from_slice(&fetched.body).context("document is not valid JSON")?;
    ensure!(
        document.get("client_id").and_then(Value::as_str) == Some(url.as_str()),
        "client_id in the document does not match its URL"
    );
    let registration = Registration::from_metadata(document, AuthMethod::None)?;
    match registration.auth_method {
        AuthMethod::None => {}
        AuthMethod::PrivateKeyJwt => check_key_source(url, &registration.metadata)?,
        _ => bail!("token_endpoint_auth_method must be none or private_key_jwt"),
    }
    Ok(registration)
}

/// A `private_key_jwt` client must publish its keys at an `https` `jwks_uri`
/// on the same host as its metadata document, and sign with an algorithm this
/// server verifies.
fn check_key_source(document_url: &Url, metadata: &Value) -> anyhow::Result<()> {
    let jwks_uri = metadata
        .get("jwks_uri")
        .and_then(Value::as_str)
        .context("private_key_jwt requires a jwks_uri")?;
    let jwks_uri = Url::parse(jwks_uri).context("jwks_uri is not a URL")?;
    ensure!(
        jwks_uri.scheme() == "https" && jwks_uri.host_str() == document_url.host_str(),
        "jwks_uri must be an https URL on the client's own host"
    );
    if let Some(alg) = metadata.get("token_endpoint_auth_signing_alg") {
        let supported = SIGNING_ALGORITHMS
            .iter()
            .any(|supported| serde_json::to_value(supported).ok().as_ref() == Some(alg));
        ensure!(
            supported,
            "token_endpoint_auth_signing_alg {alg} is not supported"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// ChatGPT's published metadata document, as of October 2026.
    #[test]
    fn accepts_chatgpts_metadata_document() {
        let document = json!({
            "client_id": "https://chatgpt.com/oauth/client.json",
            "client_uri": "https://chatgpt.com/",
            "redirect_uris": ["https://chatgpt.com/connector_platform_oauth_redirect"],
            "token_endpoint_auth_method": "private_key_jwt",
            "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "client_name": "ChatGPT",
            "logo_uri": "https://persistent.oaistatic.com/sonic/misc/openai-logo.png",
            "token_endpoint_auth_signing_alg": "RS256",
            "jwks_uri": "https://chatgpt.com/oauth/jwks.json"
        });
        let url = Url::parse(document["client_id"].as_str().unwrap()).unwrap();
        let registration = Registration::from_metadata(document, AuthMethod::None).unwrap();
        assert_eq!(registration.auth_method, AuthMethod::PrivateKeyJwt);
        assert!(check_key_source(&url, &registration.metadata).is_ok());
    }

    #[test]
    fn private_key_jwt_clients_publish_keys_on_their_own_host() {
        let document = Url::parse("https://chatgpt.com/oauth/abc/client.json").unwrap();
        let check = |metadata: Value| check_key_source(&document, &metadata).is_ok();
        assert!(check(json!({
            "jwks_uri": "https://chatgpt.com/oauth/jwks.json",
            "token_endpoint_auth_signing_alg": "RS256",
        })));
        assert!(check(
            json!({ "jwks_uri": "https://chatgpt.com/oauth/jwks.json" })
        ));
        assert!(!check(json!({})));
        assert!(!check(
            json!({ "jwks_uri": "https://evil.example/jwks.json" })
        ));
        assert!(!check(
            json!({ "jwks_uri": "http://chatgpt.com/oauth/jwks.json" })
        ));
        assert!(!check(json!({
            "jwks_uri": "https://chatgpt.com/oauth/jwks.json",
            "token_endpoint_auth_signing_alg": "HS256",
        })));
    }

    #[test]
    fn recognizes_document_urls() {
        assert!(document_url("https://claude.ai/oauth/mcp-client.json").is_some());
        for not_cimd in [
            "3f2a9c1e-0b7d-4a4e-9d55-1b2c3d4e5f60",
            "https://example.com",
            "https://example.com/",
            "http://example.com/client.json",
            "https://example.com/a/../client.json",
            "https://user@example.com/client.json",
            "https://example.com/client.json#x",
            "https://EXAMPLE.com/client.json",
        ] {
            assert!(document_url(not_cimd).is_none(), "{not_cimd}");
        }
    }
}
