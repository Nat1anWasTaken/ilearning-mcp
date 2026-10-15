//! `/files/{signed}`: short-lived download links for iLearning files that
//! tools cannot show inline. The link carries `{user_id, file_url, exp}`,
//! signed with HMAC-SHA256 under a key derived from `ENCRYPTION_KEY`, and
//! streams the file from iLearning with the user's token, which never leaves
//! the server.

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeDelta, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{config::Config, links, state::AppState};

const LINK_TTL: TimeDelta = TimeDelta::hours(1);

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Claims {
    user_id: Uuid,
    file_url: String,
    /// Unix seconds.
    exp: i64,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/files/{signed}", get(download))
}

/// A download link for `file_url`, valid for an hour.
pub fn link(state: &AppState, user_id: Uuid, file_url: &str) -> String {
    let claims = Claims {
        user_id,
        file_url: file_url.to_owned(),
        exp: (Utc::now() + LINK_TTL).timestamp(),
    };
    format!(
        "{}/files/{}",
        state.config.public_url,
        sign(&state.config, &claims)
    )
}

fn mac(config: &Config) -> Hmac<Sha256> {
    let key = Sha256::new()
        .chain_update(b"files:")
        .chain_update(config.encryption_key)
        .finalize();
    Hmac::<Sha256>::new_from_slice(&key).expect("HMAC takes any key length")
}

fn sign(config: &Config, claims: &Claims) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("serializable"));
    let tag = mac(config)
        .chain_update(payload.as_bytes())
        .finalize()
        .into_bytes();
    format!("{payload}.{}", URL_SAFE_NO_PAD.encode(tag))
}

/// The claims of a validly signed, unexpired link.
fn verify(config: &Config, signed: &str) -> Option<Claims> {
    let (payload, tag) = signed.split_once('.')?;
    let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
    mac(config)
        .chain_update(payload.as_bytes())
        .verify_slice(&tag)
        .ok()?;
    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    (claims.exp > Utc::now().timestamp()).then_some(claims)
}

async fn download(State(state): State<AppState>, Path(signed): Path<String>) -> Response {
    let Some(claims) = verify(&state.config, &signed) else {
        return (
            StatusCode::FORBIDDEN,
            "This download link is invalid or expired. Ask the agent for a new one.",
        )
            .into_response();
    };
    let linked = match links::account(&state, claims.user_id).await {
        Ok(Some(linked)) => linked,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "loading the iLearning account for a download");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match linked.account.download_response(&claims.file_url).await {
        Ok(upstream) => {
            links::mark_ok(&state.db, claims.user_id).await;
            let (filename, mime) = moodle::file_name_and_mime(&upstream);
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CONTENT_DISPOSITION, disposition(&filename)),
                    (header::CACHE_CONTROL, "private, no-store".to_owned()),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
                ],
                Body::from_stream(upstream.bytes_stream()),
            )
                .into_response()
        }
        Err(moodle::Error::InvalidToken) => {
            links::mark_invalid(&state.db, claims.user_id, &linked.token_hash).await;
            (
                StatusCode::GONE,
                "The iLearning sign-in expired. Re-link the account in the dashboard.",
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!(error = %err, "downloading an iLearning file");
            (StatusCode::BAD_GATEWAY, format!("iLearning: {err}")).into_response()
        }
    }
}

/// `attachment` with an ASCII fallback name and the RFC 5987 UTF-8 name.
fn disposition(filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| match c {
            ' '..='~' if c != '"' && c != '\\' => c,
            _ => '_',
        })
        .collect();
    let encoded =
        percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        (*crate::testing::state_config()).clone()
    }

    #[test]
    fn signed_links_verify_until_they_expire() {
        let config = config();
        let claims = Claims {
            user_id: Uuid::new_v4(),
            file_url: "https://ilearning.example/webservice/pluginfile.php/1/a.ppt".into(),
            exp: Utc::now().timestamp() + 60,
        };
        let signed = sign(&config, &claims);
        assert_eq!(verify(&config, &signed), Some(claims));

        let (payload, tag) = signed.split_once('.').unwrap();
        let mut forged = URL_SAFE_NO_PAD.decode(payload).unwrap();
        forged[10] ^= 1;
        let forged = format!("{}.{tag}", URL_SAFE_NO_PAD.encode(forged));
        assert_eq!(verify(&config, &forged), None);

        let mut other_key = config.clone();
        other_key.encryption_key = [1; 32];
        assert_eq!(verify(&other_key, &signed), None);

        let expired = Claims {
            user_id: Uuid::nil(),
            file_url: String::new(),
            exp: Utc::now().timestamp() - 1,
        };
        assert_eq!(verify(&config, &sign(&config, &expired)), None);
        assert_eq!(verify(&config, "garbage"), None);
    }

    #[test]
    fn content_disposition_escapes_names() {
        assert_eq!(
            disposition("週報 \"1\".ppt"),
            "attachment; filename=\"__ _1_.ppt\"; \
             filename*=UTF-8''%E9%80%B1%E5%A0%B1%20%221%22%2Eppt"
        );
    }
}
