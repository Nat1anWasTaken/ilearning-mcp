//! Errors returned by the Moodle layer, and the mapping from Moodle's JSON
//! exceptions onto them.

use serde_json::Value;

/// Errors returned by this crate. The `Display` text is written to be shown
/// to an AI agent or end user as-is, and never contains tokens.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The web service token expired or was revoked; the user must re-link.
    #[error(
        "the iLearning token is no longer valid; the user must re-link iLearning in the dashboard"
    )]
    InvalidToken,
    /// Moodle rejected the request with an exception.
    #[error("iLearning rejected the request ({errorcode}): {message}")]
    Moodle { errorcode: String, message: String },
    /// A pasted mobile-app link could not be parsed or verified.
    #[error("invalid iLearning link: {0}")]
    InvalidLink(String),
    /// Moodle refused because the action was performed too recently
    /// (autologin keys are limited to one per 6 minutes).
    #[error("iLearning rate limit reached; try again in a few minutes")]
    RateLimited,
    /// A transport error. Its URL is stripped (file URLs carry the token).
    #[error("could not reach iLearning: {}", full_chain(.0))]
    Http(reqwest::Error),
    /// The response did not have the expected shape.
    #[error("unexpected response from iLearning: {0}")]
    Decode(String),
    /// The request was refused locally, e.g. a file URL outside iLearning.
    #[error("not allowed: {0}")]
    NotAllowed(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

/// An error and its sources, e.g. `error sending request: connection reset`.
fn full_chain(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

impl Error {
    pub(crate) fn decode(error: impl std::fmt::Display) -> Self {
        Self::Decode(error.to_string())
    }

    /// Maps a Moodle exception (`errorcode`, `message`, optional
    /// `exception` class) onto an [`Error`].
    pub(crate) fn from_exception(errorcode: &str, message: &str, exception: &str) -> Self {
        match errorcode {
            "invalidtoken" => Self::InvalidToken,
            // Expired tokens are deleted and reported as an access exception.
            "accessexception" if message.to_ascii_lowercase().contains("token") => {
                Self::InvalidToken
            }
            "autologinkeygenerationlockout" => Self::RateLimited,
            _ => Self::Moodle {
                errorcode: if errorcode.is_empty() {
                    exception.to_owned()
                } else {
                    errorcode.to_owned()
                },
                message: message.to_owned(),
            },
        }
    }

    /// Returns the error carried by a Moodle JSON response, if it is one.
    ///
    /// Web service calls report `{exception, errorcode, message}`; file
    /// endpoints report `{error, errorcode}`.
    pub(crate) fn from_response(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if !object.contains_key("exception") && !object.contains_key("errorcode") {
            return None;
        }
        let text = |key: &str| object.get(key).and_then(Value::as_str).unwrap_or_default();
        let message = match text("message") {
            "" => text("error"),
            message => message,
        };
        Some(Self::from_exception(
            text("errorcode"),
            message,
            text("exception"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn maps_moodle_exceptions() {
        let error = Error::from_response(&json!({
            "exception": "moodle_exception", "errorcode": "invalidtoken", "message": "Invalid token"
        }));
        assert!(matches!(error, Some(Error::InvalidToken)));

        let error = Error::from_response(&json!({
            "exception": "webservice_access_exception", "errorcode": "accessexception",
            "message": "Access control exception (Invalid token - token expired)"
        }));
        assert!(matches!(error, Some(Error::InvalidToken)));

        let error = Error::from_response(&json!({
            "exception": "moodle_exception", "errorcode": "autologinkeygenerationlockout", "message": "x"
        }));
        assert!(matches!(error, Some(Error::RateLimited)));

        let error = Error::from_response(&json!({
            "error": "缺少必須要的參數(token)", "errorcode": "missingparam"
        }))
        .unwrap();
        assert_eq!(
            error.to_string(),
            "iLearning rejected the request (missingparam): 缺少必須要的參數(token)"
        );

        assert!(Error::from_response(&json!({"courses": []})).is_none());
        assert!(Error::from_response(&json!([])).is_none());
    }
}
