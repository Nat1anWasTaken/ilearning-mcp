//! Redirect URI policy: what clients may register and how requests match it.

use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

/// Checks a redirect URI at registration. Allowed are `https`, `http` on a
/// loopback host, and private-use schemes of native apps (RFC 8252 §7.1).
pub fn validate(uri: &str) -> Result<(), &'static str> {
    let url = Url::parse(uri).map_err(|_| "redirect URIs must be absolute URLs")?;
    if url.fragment().is_some() {
        return Err("redirect URIs must not contain a fragment");
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&url) => Ok(()),
        "http" => Err("http redirect URIs must use localhost, 127.0.0.1 or [::1]"),
        "javascript" | "data" | "vbscript" | "file" | "blob" | "about" => {
            Err("redirect URI scheme is not allowed")
        }
        _ => Ok(()),
    }
}

/// Whether `requested` matches the `registered` redirect URI: exact string
/// comparison, except that loopback `http` URIs may use any port (RFC 8252 §7.3).
pub fn matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    let (Ok(mut registered), Ok(mut requested)) = (Url::parse(registered), Url::parse(requested))
    else {
        return false;
    };
    registered.scheme() == "http"
        && is_loopback(&registered)
        && registered.set_port(None).is_ok()
        && requested.set_port(None).is_ok()
        && registered == requested
}

/// The host shown to the user as where an agent connects from, e.g. `claude.ai`.
pub fn origin(uri: &str) -> Option<String> {
    Url::parse(uri).ok()?.host_str().map(str::to_owned)
}

/// `uri` with authorization response parameters appended to its query.
pub fn with_params<K, V>(
    uri: &str,
    params: impl IntoIterator<Item = (K, V)>,
) -> Result<String, url::ParseError>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut url = Url::parse(uri)?;
    url.query_pairs_mut().extend_pairs(params);
    Ok(url.into())
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(ip)) => ip == Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(ip)) => ip == Ipv6Addr::LOCALHOST,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_registrations() {
        for ok in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://chatgpt.com/connector_platform_oauth_redirect",
            "http://localhost:6274/oauth/callback",
            "http://127.0.0.1:33418/callback",
            "http://[::1]/callback",
            "cursor://anysphere.cursor-retrieval/oauth/callback",
            "com.example.app:/oauth2redirect",
        ] {
            assert_eq!(validate(ok), Ok(()), "{ok}");
        }
        for bad in [
            "http://example.com/callback",
            "http://127.0.0.2/callback",
            "https://claude.ai/callback#frag",
            "javascript:alert(1)",
            "data:text/html,hi",
            "/relative/callback",
        ] {
            assert!(validate(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn matches_exactly_except_loopback_ports() {
        let claude = "https://claude.ai/api/mcp/auth_callback";
        assert!(matches(claude, claude));
        assert!(!matches(claude, "https://claude.ai/api/mcp/auth_callback/"));
        assert!(!matches(
            claude,
            "https://claude.ai:8443/api/mcp/auth_callback"
        ));
        assert!(!matches(
            claude,
            "https://claude.ai/api/mcp/auth_callback?x=1"
        ));

        assert!(matches(
            "http://127.0.0.1/callback",
            "http://127.0.0.1:51234/callback"
        ));
        assert!(matches(
            "http://localhost:6274/cb",
            "http://localhost:9999/cb"
        ));
        assert!(matches("http://[::1]:1/cb", "http://[::1]:2/cb"));
        assert!(!matches(
            "http://127.0.0.1/callback",
            "http://127.0.0.1:51234/other"
        ));
        assert!(!matches(
            "http://127.0.0.1/callback",
            "http://localhost:51234/callback"
        ));
        assert!(!matches("http://localhost/cb", "https://localhost:443/cb"));
        assert!(!matches(
            "https://localhost/cb",
            "https://localhost:8443/cb"
        ));
    }

    #[test]
    fn appends_params_and_extracts_origin() {
        assert_eq!(
            with_params(
                "https://a.example/cb?x=1",
                [("code", "c d"), ("state", "s")]
            )
            .unwrap(),
            "https://a.example/cb?x=1&code=c+d&state=s"
        );
        assert_eq!(origin("https://claude.ai/cb").as_deref(), Some("claude.ai"));
        assert_eq!(
            origin("http://localhost:3000/cb").as_deref(),
            Some("localhost")
        );
        assert_eq!(origin("com.example.app:/cb"), None);
    }
}
