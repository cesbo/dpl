//! Resolve a [`ProxyConfig`] into the concrete header + IP allowlist that the
//! nginx template needs.
//!
//! `custom` carries its data inline. `cloudflare` / `fastly` are fetched fresh
//! from the provider's public IP-list API on every deploy; a fetch or decode
//! failure aborts the deploy rather than rendering an empty allowlist.

use serde::{
    Deserialize,
    Serialize,
    de::DeserializeOwned,
};
use thiserror::Error;

use super::model::ProxyConfig;

/// Cloudflare's published IP ranges.
const CLOUDFLARE_URL: &str = "https://api.cloudflare.com/client/v4/ips";
/// Header Cloudflare sets with the original client IP.
const CLOUDFLARE_HEADER: &str = "CF-Connecting-IP";

/// Fastly's published IP ranges.
const FASTLY_URL: &str = "https://api.fastly.com/public-ip-list";
/// Header Fastly sets with the original client IP.
const FASTLY_HEADER: &str = "Fastly-Client-IP";

/// Header + trusted-proxy CIDR list ready for the nginx template
/// (`real_ip_header` and one `set_real_ip_from` per entry).
#[derive(Debug, Serialize)]
pub struct ResolvedProxy {
    pub header: String,
    pub proxies: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("fetch {provider} IP ranges from {url}")]
    Fetch {
        provider: &'static str,
        url: &'static str,
        #[source]
        source: Box<ureq::Error>,
    },

    #[error("decode {provider} IP ranges from {url}")]
    Decode {
        provider: &'static str,
        url: &'static str,
        #[source]
        source: Box<ureq::Error>,
    },
}

/// Turn a configured proxy into its rendered header + CIDR allowlist.
pub fn resolve(proxy: &ProxyConfig) -> Result<ResolvedProxy, ProxyError> {
    match proxy {
        ProxyConfig::Custom { header, proxies } => Ok(ResolvedProxy {
            header: header.clone(),
            proxies: proxies.clone(),
        }),
        ProxyConfig::Cloudflare => {
            let body: CloudflareIps = fetch("cloudflare", CLOUDFLARE_URL)?;
            let mut proxies = body.result.ipv4_cidrs;
            proxies.extend(body.result.ipv6_cidrs);
            Ok(ResolvedProxy {
                header: CLOUDFLARE_HEADER.to_string(),
                proxies,
            })
        }
        ProxyConfig::Fastly => {
            let body: FastlyIps = fetch("fastly", FASTLY_URL)?;
            let mut proxies = body.addresses;
            proxies.extend(body.ipv6_addresses);
            Ok(ResolvedProxy {
                header: FASTLY_HEADER.to_string(),
                proxies,
            })
        }
    }
}

/// `GET url` and decode the JSON body into `T`, tagging both the transport and
/// decode failures with the provider for a useful deploy error.
fn fetch<T: DeserializeOwned>(provider: &'static str, url: &'static str) -> Result<T, ProxyError> {
    let mut response = ureq::get(url).call().map_err(|source| ProxyError::Fetch {
        provider,
        url,
        source: Box::new(source),
    })?;

    response
        .body_mut()
        .read_json::<T>()
        .map_err(|source| ProxyError::Decode {
            provider,
            url,
            source: Box::new(source),
        })
}

#[derive(Debug, Deserialize)]
struct CloudflareIps {
    result: CloudflareResult,
}

#[derive(Debug, Deserialize)]
struct CloudflareResult {
    #[serde(default)]
    ipv4_cidrs: Vec<String>,
    #[serde(default)]
    ipv6_cidrs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FastlyIps {
    #[serde(default)]
    addresses: Vec<String>,
    #[serde(default)]
    ipv6_addresses: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_merges_ipv4_and_ipv6() {
        let body: CloudflareIps = serde_json::from_str(
            r#"{
                "result": {
                    "ipv4_cidrs": ["173.245.48.0/20", "103.21.244.0/22"],
                    "ipv6_cidrs": ["2400:cb00::/32"],
                    "etag": "ignored"
                },
                "success": true
            }"#,
        )
        .unwrap();

        let mut proxies = body.result.ipv4_cidrs;
        proxies.extend(body.result.ipv6_cidrs);
        assert_eq!(
            proxies,
            vec!["173.245.48.0/20", "103.21.244.0/22", "2400:cb00::/32"],
        );
    }

    #[test]
    fn fastly_merges_ipv4_and_ipv6() {
        let body: FastlyIps = serde_json::from_str(
            r#"{
                "addresses": ["23.235.32.0/20", "43.249.72.0/22"],
                "ipv6_addresses": ["2a04:4e40::/32"]
            }"#,
        )
        .unwrap();

        let mut proxies = body.addresses;
        proxies.extend(body.ipv6_addresses);
        assert_eq!(
            proxies,
            vec!["23.235.32.0/20", "43.249.72.0/22", "2a04:4e40::/32"],
        );
    }

    #[test]
    fn custom_passes_through_without_io() {
        let resolved = resolve(&ProxyConfig::Custom {
            header: "X-Forwarded-For".into(),
            proxies: vec!["192.0.2.10".into()],
        })
        .unwrap();

        assert_eq!(resolved.header, "X-Forwarded-For");
        assert_eq!(resolved.proxies, vec!["192.0.2.10"]);
    }

    #[test]
    fn fastly_missing_ipv6_defaults_empty() {
        let body: FastlyIps =
            serde_json::from_str(r#"{ "addresses": ["23.235.32.0/20"] }"#).unwrap();
        assert!(body.ipv6_addresses.is_empty());
        assert_eq!(body.addresses, vec!["23.235.32.0/20"]);
    }
}
