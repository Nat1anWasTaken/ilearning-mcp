//! Mobile-app launch links: how a user hands us a web service token.
//!
//! `admin/tool/mobile/launch.php` (with `confirmed=1`) renders a page whose
//! "launch the app" link is `moodlemobile://token=<base64>`, where the
//! payload is `md5(wwwroot + passport) ":::" token [":::" privatetoken]`.
//! The passport is a random value we choose, so the signature proves the
//! link was minted for this attempt on this site.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use md5::{Digest, Md5};
use rand::Rng;
use url::Url;

use crate::error::{Error, Result};

/// The tokens carried by a launch link.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    /// Web service token (32 hex characters).
    pub token: String,
    /// Private token, used only for renewal. Present when the link was
    /// minted with `oauthsso=1` or from a web session.
    pub privatetoken: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("token", &"<redacted>")
            .field(
                "privatetoken",
                &self.privatetoken.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A random passport (16 hex characters).
pub fn new_passport() -> String {
    let bytes: [u8; 8] = rand::rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The page that signs the user in (if needed) and shows the launch link.
/// `oauthsso` adds `oauthsso=1` (the site's Microsoft 365 issuer), which
/// makes Moodle include the private token.
pub fn launch_url(wwwroot: &Url, passport: &str, oauthsso: bool) -> Url {
    let root = wwwroot.as_str().trim_end_matches('/');
    let mut url =
        Url::parse(&format!("{root}/admin/tool/mobile/launch.php")).expect("valid launch URL");
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("service", "moodle_mobile_app")
            .append_pair("passport", passport)
            .append_pair("urlscheme", "moodlemobile")
            .append_pair("confirmed", "1");
        if oauthsso {
            query.append_pair("oauthsso", "1");
        }
    }
    url
}

/// Parses and verifies a pasted launch link. Accepts the full
/// `moodlemobile://token=…` URL, `token=…`, or the bare base64 payload,
/// tolerating surrounding whitespace, quotes, HTML entities and
/// percent-encoding.
pub fn parse(wwwroot: &Url, passport: &str, pasted: &str) -> Result<Tokens> {
    let payload = extract_payload(pasted)?;
    let decoded = decode_base64(&payload)
        .ok_or_else(|| Error::InvalidLink("the link does not contain a valid token".into()))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|_| Error::InvalidLink("the link does not contain a valid token".into()))?;
    let mut parts = decoded.split(":::");
    let signature = parts.next().unwrap_or_default();
    let token = parts.next().unwrap_or_default().trim();
    let privatetoken = parts.next().map(str::trim).filter(|t| !t.is_empty());

    let root = wwwroot.as_str().trim_end_matches('/');
    if !signature.eq_ignore_ascii_case(&md5_hex(&format!("{root}{passport}"))) {
        return Err(Error::InvalidLink(
            "the link was not created for this sign-in attempt; start again and paste the newest link"
                .into(),
        ));
    }
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Error::InvalidLink(
            "the link does not contain a valid token".into(),
        ));
    }
    if privatetoken.is_some_and(|t| !t.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return Err(Error::InvalidLink(
            "the link contains a malformed private token".into(),
        ));
    }
    Ok(Tokens {
        token: token.to_owned(),
        privatetoken: privatetoken.map(str::to_owned),
    })
}

/// The base64 payload of a pasted link.
fn extract_payload(pasted: &str) -> Result<String> {
    let mut text = unescape_entities(pasted.trim());
    if text.contains('%') {
        text = percent_encoding::percent_decode_str(&text)
            .decode_utf8_lossy()
            .into_owned();
    }
    let text = text
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '<' | '>'));
    let after = match text.rfind("token=") {
        Some(index) => &text[index + "token=".len()..],
        None if text.contains("://") => {
            return Err(Error::InvalidLink(
                "paste the link of 「若應用程式沒有自動開啟，請點選這裡」 (it starts with moodlemobile://token=)"
                    .into(),
            ));
        }
        None => text,
    };
    let payload: String = after
        .split(['&', '"', '\'', '<', '>', '?', '#'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if payload.is_empty() {
        return Err(Error::InvalidLink("the pasted text is empty".into()));
    }
    Ok(payload)
}

fn unescape_entities(text: &str) -> String {
    text.replace("&#61;", "=")
        .replace("&#x3D;", "=")
        .replace("&#x3d;", "=")
        .replace("&equals;", "=")
        .replace("&#43;", "+")
        .replace("&#47;", "/")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn decode_base64(payload: &str) -> Option<Vec<u8>> {
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(payload).ok())
}

pub(crate) fn md5_hex(input: &str) -> String {
    Md5::digest(input.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const PRIVATE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGhIjKlMnOpQrStUvWx0123";

    fn root() -> Url {
        Url::parse("https://ilearning.cycu.edu.tw").unwrap()
    }

    fn payload(passport: &str, parts: &[&str]) -> String {
        let signature = md5_hex(&format!("https://ilearning.cycu.edu.tw{passport}"));
        let mut all = vec![signature.as_str()];
        all.extend_from_slice(parts);
        STANDARD.encode(all.join(":::"))
    }

    #[test]
    fn passports_are_random_hex() {
        let a = new_passport();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, new_passport());
    }

    #[test]
    fn builds_launch_urls() {
        assert_eq!(
            launch_url(&root(), "abc", true).as_str(),
            "https://ilearning.cycu.edu.tw/admin/tool/mobile/launch.php?service=moodle_mobile_app&passport=abc&urlscheme=moodlemobile&confirmed=1&oauthsso=1"
        );
        assert!(
            !launch_url(&root(), "abc", false)
                .as_str()
                .contains("oauthsso")
        );
    }

    #[test]
    fn parses_every_pasted_form() {
        let p = payload("pp", &[TOKEN, PRIVATE]);
        let expected = Tokens {
            token: TOKEN.into(),
            privatetoken: Some(PRIVATE.into()),
        };
        for pasted in [
            format!("moodlemobile://token={p}"),
            format!("  moodlemobile://token={p}\n"),
            format!("token={p}"),
            p.clone(),
            format!("\"moodlemobile://token={}\"", p.replace('=', "&#61;")),
            format!(
                "moodlemobile://token={}",
                p.replace('=', "%3D").replace('+', "%2B")
            ),
            format!("moodlemobile://token={}\n{}", &p[..10], &p[10..]),
        ] {
            assert_eq!(parse(&root(), "pp", &pasted).unwrap(), expected, "{pasted}");
        }
    }

    #[test]
    fn private_token_is_optional() {
        let p = payload("pp", &[TOKEN]);
        let tokens = parse(
            &Url::parse("https://ilearning.cycu.edu.tw/").unwrap(),
            "pp",
            &p,
        )
        .unwrap();
        assert_eq!(tokens.token, TOKEN);
        assert_eq!(tokens.privatetoken, None);
    }

    #[test]
    fn rejects_bad_links() {
        let p = payload("pp", &[TOKEN, PRIVATE]);
        assert!(matches!(
            parse(&root(), "other", &p),
            Err(Error::InvalidLink(_))
        ));
        let elsewhere = Url::parse("https://moodle.example.com").unwrap();
        assert!(matches!(
            parse(&elsewhere, "pp", &p),
            Err(Error::InvalidLink(_))
        ));
        assert!(matches!(
            parse(&root(), "pp", ""),
            Err(Error::InvalidLink(_))
        ));
        assert!(matches!(
            parse(&root(), "pp", "not base64!!"),
            Err(Error::InvalidLink(_))
        ));
        assert!(matches!(
            parse(&root(), "pp", "https://ilearning.cycu.edu.tw/my/"),
            Err(Error::InvalidLink(_))
        ));
        let short = payload("pp", &["abc"]);
        assert!(matches!(
            parse(&root(), "pp", &short),
            Err(Error::InvalidLink(_))
        ));
    }

    #[test]
    fn debug_hides_tokens() {
        let tokens = Tokens {
            token: TOKEN.into(),
            privatetoken: Some(PRIVATE.into()),
        };
        let debug = format!("{tokens:?}");
        assert!(!debug.contains(TOKEN) && !debug.contains(PRIVATE));
    }
}
