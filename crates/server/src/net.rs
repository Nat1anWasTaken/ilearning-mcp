//! Outbound HTTP to URLs chosen by OAuth clients or agents, guarded against SSRF.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};
use url::{Host, Url};

pub struct Fetched {
    pub body: Vec<u8>,
}

/// GETs an `http(s)` URL that must live on the public internet.
///
/// Every address the host resolves to is checked at connection time (so DNS
/// rebinding cannot sneak past the check), redirects are not followed, proxies
/// are bypassed, the whole exchange is bounded by `timeout` and the body by
/// `max_bytes`. Non-2xx responses are errors.
pub async fn fetch_public_url(
    url: &Url,
    max_bytes: usize,
    timeout: Duration,
) -> anyhow::Result<Fetched> {
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "only http and https URLs can be fetched"
    );
    // IP literals never reach the resolver, so check them here.
    let literal = match url.host() {
        Some(Host::Domain(_)) => None,
        Some(Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
        None => bail!("URL has no host"),
    };
    if literal.is_some_and(|ip| !is_public(ip)) {
        bail!("refusing to fetch a non-public address");
    }

    // A dedicated client per call: fetches are rare and the resolver is the guard.
    let client = reqwest::Client::builder()
        .dns_resolver(PublicResolver)
        .redirect(redirect::Policy::none())
        .no_proxy()
        .timeout(timeout)
        .build()
        .context("building HTTP client")?;
    let mut response = client.get(url.clone()).send().await?;
    let status = response.status();
    ensure!(status.is_success(), "server responded with {status}");
    if response
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        bail!("response is larger than {max_bytes} bytes");
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= max_bytes,
            "response is larger than {max_bytes} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(Fetched { body })
}

/// Resolves through the system resolver and drops every non-public address.
struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{} has no public address", name.as_str()).into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => is_public_v4(ip),
            None => is_public_v6(ip),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || a == 0 // "this network"
        || (a == 100 && b & 0xc0 == 64) // carrier-grade NAT, 100.64.0.0/10
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && b & 0xfe == 18) // benchmarking, 198.18.0.0/15
        || a >= 240) // reserved
}

/// Only global unicast (`2000::/3`), minus ranges that are reserved or embed
/// an arbitrary IPv4 address (Teredo, 6to4).
fn is_public_v6(ip: Ipv6Addr) -> bool {
    let [s0, s1, ..] = ip.segments();
    s0 & 0xe000 == 0x2000
        && !(s0 == 0x2001 && (s1 == 0 || s1 == 0x0db8)) // Teredo, documentation
        && s0 != 0x2002 // 6to4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:a00:1::",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip} must not be public");
        }
        for ip in [
            "1.1.1.1",
            "140.82.112.3",
            "100.128.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip} must be public");
        }
    }

    #[tokio::test]
    async fn refuses_private_targets() {
        for url in [
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://localhost/",
            "ftp://example.com/",
        ] {
            let url = Url::parse(url).unwrap();
            assert!(
                fetch_public_url(&url, 1024, Duration::from_secs(5))
                    .await
                    .is_err(),
                "{url} must be refused"
            );
        }
    }
}
