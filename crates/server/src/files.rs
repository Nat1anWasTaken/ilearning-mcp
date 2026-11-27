//! Files in tool results, and `/files/{signed}`: short-lived download links
//! for iLearning files that tools cannot show inline. The link carries
//! `{user_id, file_url, exp}`, signed with HMAC-SHA256 under a key derived
//! from `ENCRYPTION_KEY`, and streams the file from iLearning with the
//! user's token, which never leaves the server.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeDelta, Utc};
use hmac::{Hmac, Mac};
use mcp_backbone::{
    AppState, CallError, Output,
    config::Config,
    rmcp::model::{ContentBlock, ResourceContents},
};
use moodle::{FileChunk, FileContent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::service::ILearningService;

const LINK_TTL: TimeDelta = TimeDelta::hours(1);

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Claims {
    user_id: Uuid,
    file_url: String,
    /// Unix seconds.
    exp: i64,
}

/// Signs download links for one user's files.
#[derive(Clone)]
pub struct Signer {
    user_id: Uuid,
    key: [u8; 32],
    public_url: String,
}

impl Signer {
    pub fn new(config: &Config, user_id: Uuid) -> Self {
        Self {
            user_id,
            key: link_key(config),
            public_url: config.public_url.clone(),
        }
    }

    #[cfg(test)]
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    /// A download link for `file_url`, valid for an hour.
    pub fn link(&self, file_url: &str) -> String {
        let claims = Claims {
            user_id: self.user_id,
            file_url: file_url.to_owned(),
            exp: (Utc::now() + LINK_TTL).timestamp(),
        };
        format!("{}/files/{}", self.public_url, sign(&self.key, &claims))
    }
}

/// The links' HMAC key, kept apart from the sealing key it derives from.
fn link_key(config: &Config) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"files:")
        .chain_update(config.encryption_key)
        .finalize()
        .into()
}

fn mac(key: &[u8; 32]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length")
}

fn sign(key: &[u8; 32], claims: &Claims) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("serializable"));
    let tag = mac(key)
        .chain_update(payload.as_bytes())
        .finalize()
        .into_bytes();
    format!("{payload}.{}", URL_SAFE_NO_PAD.encode(tag))
}

/// The claims of a validly signed, unexpired link.
fn verify(key: &[u8; 32], signed: &str) -> Option<Claims> {
    let (payload, tag) = signed.split_once('.')?;
    let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
    mac(key)
        .chain_update(payload.as_bytes())
        .verify_slice(&tag)
        .ok()?;
    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    (claims.exp > Utc::now().timestamp()).then_some(claims)
}

/// `GET /files/{signed}`: streams the file from iLearning.
pub async fn download(
    State(state): State<AppState<ILearningService>>,
    Path(signed): Path<String>,
) -> Response {
    let Some(claims) = verify(&link_key(&state.config), &signed) else {
        return (
            StatusCode::FORBIDDEN,
            "This download link is invalid or expired. Ask the agent for a new one.",
        )
            .into_response();
    };
    let file_url = claims.file_url;
    let downloaded = state
        .call(claims.user_id, |account| {
            let file_url = file_url.clone();
            async move { account.moodle.download_response(&file_url).await }
        })
        .await;
    match downloaded {
        Ok(upstream) => {
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
        Err(CallError::NotLinked) => StatusCode::NOT_FOUND.into_response(),
        Err(CallError::NeedsRelink | CallError::Upstream(moodle::Error::InvalidToken)) => (
            StatusCode::GONE,
            "The iLearning sign-in expired. Re-link the account in the dashboard.",
        )
            .into_response(),
        Err(CallError::Upstream(err)) => {
            tracing::warn!(error = %err, "downloading an iLearning file");
            (StatusCode::BAD_GATEWAY, format!("iLearning: {err}")).into_response()
        }
        Err(CallError::Internal(err)) => {
            tracing::error!(error = ?err, "loading the iLearning account for a download");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
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

/// A file read by `read_file`, for the agent: text, an image, or a download
/// link for anything else. The call log keeps everything but the content.
pub fn file_output(file: FileContent, signer: &Signer) -> Output {
    match file {
        FileContent::Text {
            filename,
            mime,
            text,
            truncated,
        } => {
            let header = json!({
                "filename": filename,
                "mime": mime,
                "chars": text.chars().count(),
                "truncated": truncated,
            });
            Output::Content {
                content: vec![
                    ContentBlock::text(header.to_string()),
                    ContentBlock::text(text),
                ],
                logged: Some(header),
            }
        }
        FileContent::Image {
            filename,
            mime,
            bytes,
        } => {
            let logged = json!({ "filename": filename, "mime": mime, "size": bytes.len() });
            Output::Content {
                content: vec![
                    ContentBlock::text(logged.to_string()),
                    ContentBlock::image(STANDARD.encode(&bytes), mime),
                ],
                logged: Some(logged),
            }
        }
        FileContent::Binary {
            filename,
            mime,
            size,
            file_url,
        } => {
            let download_url = signer.link(&file_url);
            let logged = json!({ "filename": filename, "mime": mime, "size": size });
            let text = format!(
                "{filename} ({mime}, {size} bytes) is not returned as text or an image. \
                 Download link to the original file, valid for one hour, to give the user \
                 or fetch: {download_url}"
            );
            Output::Content {
                content: vec![ContentBlock::text(text)],
                logged: Some(logged),
            }
        }
    }
}

/// A part of a file from `download_file`, as an embedded resource. The call
/// log keeps its description only.
pub fn chunk_output(chunk: FileChunk) -> Output {
    let FileChunk {
        filename,
        mime,
        file_url,
        size,
        offset,
        bytes,
        next_offset,
    } = chunk;
    let header = json!({
        "filename": filename,
        "mime": mime,
        "size": size,
        "offset": offset,
        "length": bytes.len(),
        "next_offset": next_offset,
        "encoding": "base64",
    });
    let blob = ResourceContents::blob(STANDARD.encode(&bytes), file_url).with_mime_type(mime);
    Output::Content {
        content: vec![
            ContentBlock::text(header.to_string()),
            ContentBlock::resource(blob),
        ],
        logged: Some(header),
    }
}

#[cfg(test)]
mod tests {
    use mcp_backbone::{rmcp::model::CallToolResult, testing::PUBLIC_URL};
    use serde_json::Value;

    use super::*;

    fn config() -> Config {
        Config::for_tests(PUBLIC_URL)
    }

    fn parts(output: Output) -> (CallToolResult, Option<Value>) {
        output.into_parts()
    }

    fn text(result: &CallToolResult, index: usize) -> &str {
        &result.content[index].as_text().unwrap().text
    }

    #[test]
    fn signed_links_verify_until_they_expire() {
        let config = config();
        let key = link_key(&config);
        let claims = Claims {
            user_id: Uuid::new_v4(),
            file_url: "https://ilearning.example/webservice/pluginfile.php/1/a.ppt".into(),
            exp: Utc::now().timestamp() + 60,
        };
        let signed = sign(&key, &claims);
        assert_eq!(verify(&key, &signed), Some(claims));

        let (payload, tag) = signed.split_once('.').unwrap();
        let mut forged = URL_SAFE_NO_PAD.decode(payload).unwrap();
        forged[10] ^= 1;
        let forged = format!("{}.{tag}", URL_SAFE_NO_PAD.encode(forged));
        assert_eq!(verify(&key, &forged), None);

        let mut other_key = config.clone();
        other_key.encryption_key = [1; 32];
        assert_eq!(verify(&link_key(&other_key), &signed), None);

        let expired = Claims {
            user_id: Uuid::nil(),
            file_url: String::new(),
            exp: Utc::now().timestamp() - 1,
        };
        assert_eq!(verify(&key, &sign(&key, &expired)), None);
        assert_eq!(verify(&key, "garbage"), None);

        // The signer's links verify and name the user.
        let user_id = Uuid::new_v4();
        let link = Signer::new(&config, user_id).link("https://x.test/f");
        let signed = link.strip_prefix(&format!("{PUBLIC_URL}/files/")).unwrap();
        let claims = verify(&key, signed).unwrap();
        assert_eq!(
            (claims.user_id, claims.file_url.as_str()),
            (user_id, "https://x.test/f")
        );
    }

    #[test]
    fn content_disposition_escapes_names() {
        assert_eq!(
            disposition("週報 \"1\".ppt"),
            "attachment; filename=\"__ _1_.ppt\"; \
             filename*=UTF-8''%E9%80%B1%E5%A0%B1%20%221%22%2Eppt"
        );
    }

    #[test]
    fn files_are_logged_without_their_content() {
        let signer = Signer::new(&config(), Uuid::new_v4());
        let file = FileContent::Text {
            filename: "a.pdf".into(),
            mime: "application/pdf".into(),
            text: "hello".into(),
            truncated: false,
        };
        let (result, logged) = parts(file_output(file, &signer));
        assert_eq!(result.is_error, Some(false));
        assert_eq!(text(&result, 1), "hello");
        assert_eq!(
            logged,
            Some(
                json!({ "filename": "a.pdf", "mime": "application/pdf", "chars": 5, "truncated": false })
            )
        );

        let file = FileContent::Image {
            filename: "a.png".into(),
            mime: "image/png".into(),
            bytes: vec![1, 2, 3],
        };
        let (result, logged) = parts(file_output(file, &signer));
        assert!(result.content[1].as_image().is_some());
        assert_eq!(logged.unwrap()["size"], 3);

        let file = FileContent::Binary {
            filename: "a.ppt".into(),
            mime: "application/vnd.ms-powerpoint".into(),
            size: 10,
            file_url: "https://ilearning.example/webservice/pluginfile.php/1/a.ppt".into(),
        };
        let (result, logged) = parts(file_output(file, &signer));
        assert!(
            text(&result, 0).contains(&format!("{PUBLIC_URL}/files/")),
            "{}",
            text(&result, 0)
        );
        assert_eq!(logged.unwrap()["filename"], "a.ppt");

        let chunk = FileChunk {
            filename: "a.docx".into(),
            mime: "application/octet-stream".into(),
            file_url: "https://ilearning.example/webservice/pluginfile.php/1/a.docx".into(),
            size: Some(10),
            offset: 4,
            bytes: vec![1, 2, 3],
            next_offset: Some(7),
        };
        let (result, logged) = parts(chunk_output(chunk));
        let resource = result.content[1].as_resource().unwrap();
        let ResourceContents::BlobResourceContents { blob, .. } = &resource.resource else {
            panic!("not a blob");
        };
        assert_eq!(blob, "AQID");
        assert_eq!(
            logged,
            Some(json!({
                "filename": "a.docx", "mime": "application/octet-stream", "size": 10,
                "offset": 4, "length": 3, "next_offset": 7, "encoding": "base64",
            }))
        );
        assert_eq!(text(&result, 0), logged.unwrap().to_string());
    }
}
