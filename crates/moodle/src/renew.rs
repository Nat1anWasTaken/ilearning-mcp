//! Token renewal through a short-lived web session.
//!
//! 1. [`autologin`] trades the private token for a one-time autologin key
//!    (`tool_mobile_get_autologin_key`, at most once per 6 minutes) and
//!    opens a web session with it. **The site allows one web session per
//!    user, so this signs the user out of iLearning in their browser.**
//! 2. [`touch`] keeps the session alive (idle timeout is one hour).
//! 3. [`mint`] opens `launch.php` in the session and reads the launch link,
//!    which carries the current token (a new one once the old expired) and
//!    a private token.

use std::sync::Mutex;

use reqwest::header::{COOKIE, LOCATION, SET_COOKIE, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use url::Url;

use crate::Moodle;
use crate::error::{Error, Result};
use crate::link::{self, Tokens};

/// Browser-like user agent for web session pages.
const BROWSER_AGENT: &str = "Mozilla/5.0 (compatible; ilearning-mcp)";
const MAX_REDIRECTS: usize = 8;

/// An authenticated Moodle web session (its cookie jar). Never printed.
pub struct WebSession {
    cookies: Mutex<Vec<(String, String)>>,
    sesskey: Mutex<Option<String>>,
}

impl std::fmt::Debug for WebSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSession").finish_non_exhaustive()
    }
}

impl WebSession {
    fn new() -> Self {
        Self {
            cookies: Mutex::new(Vec::new()),
            sesskey: Mutex::new(None),
        }
    }

    fn cookie_header(&self) -> String {
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn has_cookie(&self, name: &str) -> bool {
        self.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|(k, _)| k == name)
    }

    /// Applies `Set-Cookie` headers (deleted cookies are removed).
    fn store(&self, response: &reqwest::Response) {
        let mut jar = self.cookies.lock().unwrap_or_else(|e| e.into_inner());
        for header in response.headers().get_all(SET_COOKIE) {
            let Ok(header) = header.to_str() else {
                continue;
            };
            let Some((name, value)) = parse_set_cookie(header) else {
                continue;
            };
            jar.retain(|(k, _)| *k != name);
            if !value.is_empty() && value != "deleted" {
                jar.push((name, value));
            }
        }
    }
}

/// `name=value` of a `Set-Cookie` header, or `None` when it expires the
/// cookie.
fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let mut attributes = header.split(';');
    let (name, value) = attributes.next()?.split_once('=')?;
    let expired = attributes.any(|a| {
        let a = a.trim().to_ascii_lowercase();
        a == "max-age=0" || a.starts_with("max-age=-") || a.contains("expires=thu, 01-jan-1970")
    });
    let value = if expired { "" } else { value.trim() };
    Some((name.trim().to_owned(), value.to_owned()))
}

/// Opens a web session as the token's owner. Signs the user out of
/// iLearning in their browser. Fails with [`Error::RateLimited`] if a key
/// was requested less than 6 minutes ago.
pub async fn autologin(
    m: &Moodle,
    token: &str,
    privatetoken: &str,
    userid: i64,
) -> Result<WebSession> {
    // The private token travels only in the POST body.
    let key = m
        .account(token.to_owned())
        .call(
            "tool_mobile_get_autologin_key",
            json!({ "privatetoken": privatetoken }),
        )
        .await?;
    let (Some(key), Some(autologin_url)) = (
        key.get("key").and_then(Value::as_str),
        key.get("autologinurl").and_then(Value::as_str),
    ) else {
        return Err(Error::decode("autologin key response has no key"));
    };
    let mut url = Url::parse(autologin_url).map_err(Error::decode)?;
    check_same_site(m, &url)?;
    url.query_pairs_mut()
        .append_pair("userid", &userid.to_string())
        .append_pair("key", key);

    let session = WebSession::new();
    // The autologin endpoint only accepts the mobile app's user agent
    // (the client default).
    let response = m.no_redirect.get(url).send().await?;
    session.store(&response);
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|l| l.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !response.status().is_redirection() || !session.has_cookie("MoodleSession") {
        return Err(Error::Decode(format!(
            "autologin did not start a session (HTTP {})",
            response.status()
        )));
    }
    if location.contains("/login/") {
        return Err(Error::Moodle {
            errorcode: "autologinfailed".into(),
            message: "the autologin key was rejected".into(),
        });
    }
    Ok(session)
}

/// Keeps the session alive (`core_session_touch`).
pub async fn touch(m: &Moodle, s: &WebSession) -> Result<()> {
    let sesskey = sesskey(m, s).await?;
    let mut url = m.endpoint("/lib/ajax/service.php");
    url.query_pairs_mut()
        .append_pair("sesskey", &sesskey)
        .append_pair("info", "core_session_touch");
    let response = m
        .no_redirect
        .post(url)
        .header(USER_AGENT, BROWSER_AGENT)
        .header(COOKIE, s.cookie_header())
        .json(&json!([{ "index": 0, "methodname": "core_session_touch", "args": {} }]))
        .send()
        .await?;
    s.store(&response);
    let body: Value = response.error_for_status()?.json().await?;
    ajax_result(&body).map(|_| ())
}

/// Reads the launch link in the session, returning the current token and
/// private token.
pub async fn mint(m: &Moodle, s: &WebSession) -> Result<Tokens> {
    let passport = link::new_passport();
    let url = link::launch_url(m.wwwroot(), &passport, false);
    let (status, html) = session_get(m, s, url).await?;
    if status != StatusCode::OK {
        return Err(Error::Decode(format!("launch page returned HTTP {status}")));
    }
    let href = launchapp_href(&html)
        .ok_or_else(|| Error::decode("the launch page has no app link (session expired?)"))?;
    link::parse(m.wwwroot(), &passport, &href)
}

/// The sesskey of the session, scraped from a page once.
async fn sesskey(m: &Moodle, s: &WebSession) -> Result<String> {
    if let Some(key) = s.sesskey.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(key);
    }
    let (_, html) = session_get(m, s, m.endpoint("/user/preferences.php")).await?;
    let key = scrape_sesskey(&html)
        .ok_or_else(|| Error::decode("no sesskey in page (session expired?)"))?;
    *s.sesskey.lock().unwrap_or_else(|e| e.into_inner()) = Some(key.clone());
    Ok(key)
}

/// GETs a page in the session, following same-site redirects manually so
/// cookies set along the way are kept. Fails if Moodle sends the session to
/// the login page.
async fn session_get(m: &Moodle, s: &WebSession, mut url: Url) -> Result<(StatusCode, String)> {
    for _ in 0..MAX_REDIRECTS {
        check_same_site(m, &url)?;
        if url.path().contains("/login/") {
            return Err(Error::Moodle {
                errorcode: "sessionexpired".into(),
                message: "the web session is no longer signed in".into(),
            });
        }
        let response = m
            .no_redirect
            .request(Method::GET, url.clone())
            .header(USER_AGENT, BROWSER_AGENT)
            .header(COOKIE, s.cookie_header())
            .send()
            .await?;
        s.store(&response);
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|l| l.to_str().ok())
                .ok_or_else(|| Error::decode("redirect without location"))?;
            url = url.join(location).map_err(Error::decode)?;
            continue;
        }
        let status = response.status();
        return Ok((status, response.text().await?));
    }
    Err(Error::decode("too many redirects"))
}

fn check_same_site(m: &Moodle, url: &Url) -> Result<()> {
    if url.origin() == m.wwwroot().origin() {
        Ok(())
    } else {
        Err(Error::NotAllowed(format!(
            "{} is not on iLearning",
            url.origin().ascii_serialization()
        )))
    }
}

/// The first result of a `lib/ajax/service.php` response.
fn ajax_result(body: &Value) -> Result<&Value> {
    let first =
        body.as_array().and_then(|a| a.first()).ok_or_else(|| {
            match Error::from_response(body) {
                Some(error) => error,
                None => Error::decode("empty AJAX response"),
            }
        })?;
    if first.get("error").and_then(Value::as_bool) == Some(true) {
        let exception = first.get("exception").unwrap_or(&Value::Null);
        return Err(
            Error::from_response(exception).unwrap_or_else(|| Error::Moodle {
                errorcode: "ajaxerror".into(),
                message: exception.to_string(),
            }),
        );
    }
    Ok(first.get("data").unwrap_or(&Value::Null))
}

/// `"sesskey":"…"` from Moodle's `M.cfg` in any page.
fn scrape_sesskey(html: &str) -> Option<String> {
    let start = html.find("\"sesskey\":\"")? + "\"sesskey\":\"".len();
    let key: String = html[start..].chars().take_while(|c| *c != '"').collect();
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())).then_some(key)
}

/// The `href` of `<a id="launchapp" …>`, with HTML entities decoded.
pub(crate) fn launchapp_href(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(start) = rest.find("<a") {
        let tag_body = &rest[start..];
        let end = tag_body.find('>')?;
        let tag = &tag_body[..end];
        if attribute(tag, "id").as_deref() == Some("launchapp") {
            return attribute(tag, "href").map(|href| {
                href.replace("&amp;", "&")
                    .replace("&#61;", "=")
                    .replace("&#x3D;", "=")
                    .replace("&quot;", "\"")
            });
        }
        rest = &tag_body[end..];
    }
    None
}

/// The value of attribute `name` in an opening tag (quoted or not).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut search = 0;
    while let Some(found) = tag[search..].find(name) {
        let index = search + found;
        search = index + name.len();
        let preceded = index > 0 && bytes[index - 1].is_ascii_whitespace();
        let rest = tag[search..].trim_start();
        if !preceded || !rest.starts_with('=') {
            continue;
        }
        let value = rest[1..].trim_start();
        return Some(match value.chars().next()? {
            quote @ ('"' | '\'') => value[1..].split(quote).next()?.to_owned(),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()?
                .to_owned(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCH_PAGE: &str = r#"<!DOCTYPE html>
<html dir="ltr" lang="zh-tw"><head><title>i-learning</title>
<script>M.cfg = {"wwwroot":"https:\/\/ilearning.cycu.edu.tw","sesskey":"AbC123xyZ9","sessiontimeout":"3600"};</script>
</head><body>
<a class="btn" href="https://ilearning.cycu.edu.tw/">首頁</a>
<p><a class="btn btn-primary" id="launchapp" href="moodlemobile://token=YWJj&amp;x=1">若應用程式沒有自動開啟，請點選這裡</a></p>
</body></html>"#;

    #[test]
    fn finds_the_launch_link() {
        assert_eq!(
            launchapp_href(LAUNCH_PAGE).as_deref(),
            Some("moodlemobile://token=YWJj&x=1")
        );
        let reversed = r#"<a href='moodlemobile://token=Zm9v' class="x" id='launchapp'>click here to launch the app</a>"#;
        assert_eq!(
            launchapp_href(reversed).as_deref(),
            Some("moodlemobile://token=Zm9v")
        );
        assert_eq!(launchapp_href("<a id=\"other\" href=\"x\">"), None);
        assert_eq!(launchapp_href("<html>login page</html>"), None);
    }

    #[test]
    fn launch_page_round_trip() {
        use base64::Engine;
        let root = Url::parse("https://ilearning.cycu.edu.tw").unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        let payload = format!(
            "{}:::{token}:::PRIVATE0123",
            link::md5_hex("https://ilearning.cycu.edu.twpass")
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
        let page = LAUNCH_PAGE.replace("YWJj&amp;x=1", &encoded);
        let tokens = link::parse(&root, "pass", &launchapp_href(&page).unwrap()).unwrap();
        assert_eq!(tokens.token, token);
        assert_eq!(tokens.privatetoken.as_deref(), Some("PRIVATE0123"));
    }

    #[test]
    fn scrapes_sesskey() {
        assert_eq!(scrape_sesskey(LAUNCH_PAGE).as_deref(), Some("AbC123xyZ9"));
        assert_eq!(scrape_sesskey("<html></html>"), None);
    }

    #[test]
    fn parses_set_cookie() {
        assert_eq!(
            parse_set_cookie("MoodleSession=abc123; path=/; secure; HttpOnly; SameSite=None"),
            Some(("MoodleSession".into(), "abc123".into()))
        );
        assert_eq!(
            parse_set_cookie(
                "MOODLEID1_=deleted; expires=Thu, 01-Jan-1970 00:00:01 GMT; Max-Age=0"
            ),
            Some(("MOODLEID1_".into(), String::new()))
        );
    }

    #[test]
    fn reads_ajax_results() {
        let ok = serde_json::json!([{"error": false, "data": true}]);
        assert_eq!(ajax_result(&ok).unwrap(), &Value::Bool(true));
        let expired = serde_json::json!([{"error": true, "exception": {
            "message": "您的連線階段已經逾時", "errorcode": "servicerequireslogin", "link": "", "moreinfourl": ""
        }}]);
        assert!(matches!(
            ajax_result(&expired),
            Err(Error::Moodle { errorcode, .. }) if errorcode == "servicerequireslogin"
        ));
    }
}
