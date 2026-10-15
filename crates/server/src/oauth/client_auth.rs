//! Client authentication at the token and revocation endpoints (RFC 6749 §2.3,
//! RFC 7523 §2.2). Each client must use the one method it registered.

use axum::http::{HeaderMap, header};
use base64::{Engine, engine::general_purpose::STANDARD};
use percent_encoding::percent_decode_str;
use serde::Deserialize;

use super::{
    OAuthError,
    client::{self, AuthMethod, Client},
    client_assertion::ASSERTION_TYPE,
};
use crate::{crypto::hash_token, state::AppState};

/// Client credentials sent in a form body, for `#[serde(flatten)]` into a request.
#[derive(Default, Deserialize)]
pub struct FormCredentials {
    client_id: Option<String>,
    client_secret: Option<String>,
    client_assertion_type: Option<String>,
    client_assertion: Option<String>,
}

impl FormCredentials {
    fn assertion(&self) -> Result<Option<&str>, OAuthError> {
        match (
            self.client_assertion_type.as_deref(),
            self.client_assertion.as_deref(),
        ) {
            (None, None) => Ok(None),
            (Some(ASSERTION_TYPE), Some(assertion)) => Ok(Some(assertion)),
            _ => Err(OAuthError::invalid_request(
                "client_assertion requires client_assertion_type jwt-bearer",
            )),
        }
    }
}

/// Identifies the client and checks the credentials of the method it registered.
pub async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    form: &FormCredentials,
) -> Result<Client, OAuthError> {
    let basic = basic_credentials(headers)?;
    let assertion = form.assertion()?;
    let client_id = match (&basic, form.client_id.as_deref(), assertion) {
        (Some(_), _, Some(_)) => {
            return Err(OAuthError::invalid_request(
                "use only one client authentication method",
            ));
        }
        (Some((id, _)), Some(form_id), _) if id != form_id => {
            return Err(OAuthError::invalid_request(
                "client_id does not match the Authorization header",
            ));
        }
        (Some((id, _)), _, _) => id.clone(),
        (None, Some(id), _) => id.to_owned(),
        // RFC 7523 lets the assertion alone identify the client; its claims
        // are only trusted once the signature is verified below.
        (None, None, Some(assertion)) => assertion_subject(assertion)?,
        (None, None, None) => return Err(OAuthError::invalid_client()),
    };
    let secret = match &basic {
        Some((_, secret)) => Some((AuthMethod::ClientSecretBasic, secret.as_str())),
        None => form
            .client_secret
            .as_deref()
            .map(|secret| (AuthMethod::ClientSecretPost, secret)),
    };

    let client = client::find(&state.db, &client_id)
        .await?
        .ok_or_else(OAuthError::invalid_client)?;
    match (client.token_endpoint_auth_method, assertion) {
        (AuthMethod::PrivateKeyJwt, Some(assertion)) => {
            let jwks_uri = client
                .jwks_uri
                .as_deref()
                .ok_or_else(OAuthError::invalid_client)?;
            let issuer = &state.config.public_url;
            let audiences = [
                issuer.clone(),
                format!("{issuer}/oauth/token"),
                format!("{issuer}/oauth/revoke"),
            ];
            state
                .client_assertions
                .verify(&client.client_id, jwks_uri, assertion, &audiences)
                .await?;
            Ok(client)
        }
        // An assertion from a client registered otherwise is refused rather
        // than ignored.
        (_, Some(_)) | (AuthMethod::PrivateKeyJwt, None) => Err(OAuthError::invalid_client()),
        (AuthMethod::None, None) => Ok(client),
        (expected, None) => match (secret, &client.client_secret_hash) {
            (Some((method, secret)), Some(hash))
                if method == expected && hash_token(secret) == *hash =>
            {
                Ok(client)
            }
            _ => Err(OAuthError::invalid_client()),
        },
    }
}

/// The unverified `sub` of an assertion, used only to look its client up.
fn assertion_subject(assertion: &str) -> Result<String, OAuthError> {
    #[derive(Deserialize)]
    struct Subject {
        sub: String,
    }
    jsonwebtoken::dangerous::insecure_decode_claims::<Subject>(assertion)
        .map(|claims| claims.sub)
        .map_err(|_| OAuthError::invalid_client())
}

/// `Authorization: Basic` credentials, form-urlencoded inside base64.
fn basic_credentials(headers: &HeaderMap) -> Result<Option<(String, String)>, OAuthError> {
    let Some((scheme, encoded)) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
    else {
        return Ok(None);
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return Ok(None);
    }
    let decoded = STANDARD
        .decode(encoded.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(OAuthError::invalid_client)?;
    let (id, secret) = decoded
        .split_once(':')
        .ok_or_else(OAuthError::invalid_client)?;
    Ok(Some((form_decode(id)?, form_decode(secret)?)))
}

fn form_decode(value: &str) -> Result<String, OAuthError> {
    percent_decode_str(&value.replace('+', " "))
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| OAuthError::invalid_client())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(authorization).unwrap(),
        );
        headers
    }

    #[test]
    fn decodes_basic_credentials() {
        let encoded = STANDARD.encode("my%20client:s%3Acret+x");
        assert_eq!(
            basic_credentials(&headers(&format!("basic {encoded}"))).unwrap(),
            Some(("my client".into(), "s:cret x".into()))
        );
        assert_eq!(basic_credentials(&headers("Bearer abc")).unwrap(), None);
        assert_eq!(basic_credentials(&HeaderMap::new()).unwrap(), None);
        assert!(basic_credentials(&headers("Basic !!!")).is_err());
    }
}
