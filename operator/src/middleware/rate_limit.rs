//! Rate limiting middleware with proxy-aware key extraction.
//!
//! X-Forwarded-For and similar headers are ONLY trusted when the connecting peer
//! is a loopback or RFC-1918 private address (i.e., a trusted internal proxy).
//! Direct connections from public IPs always use the peer address — accepting
//! client-supplied forwarded headers from untrusted peers allows IP spoofing to
//! trivially bypass per-IP rate limits.

use axum::http::Request;
use std::net::{IpAddr, SocketAddr};
use tower_governor::{key_extractor::KeyExtractor, GovernorError};

/// Returns true when the IP is a loopback or RFC-1918 private address.
fn is_trusted_proxy(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Custom key extractor.
/// Forwarded headers are only honoured for requests arriving from trusted (private/loopback) proxies.
#[derive(Clone)]
pub struct ProxyAwareKeyExtractor;

impl KeyExtractor for ProxyAwareKeyExtractor {
    type Key = String;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        // Determine peer address first.
        let peer_addr = req.extensions().get::<SocketAddr>().copied();
        let peer_ip = peer_addr.map(|a| a.ip());

        // Only trust forwarded headers when the direct connection is from a trusted proxy.
        let from_trusted_proxy = peer_ip.map(|ip| is_trusted_proxy(&ip)).unwrap_or(false);

        if from_trusted_proxy {
            // X-Forwarded-For: client, proxy1, proxy2 — leftmost is the original client
            if let Some(header_value) = req.headers().get("X-Forwarded-For") {
                if let Ok(header_str) = header_value.to_str() {
                    if let Some(client_ip) = header_str.split(',').next() {
                        let ip = client_ip.trim();
                        if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }

            // Forwarded header (RFC 7239)
            if let Some(header_value) = req.headers().get("Forwarded") {
                if let Ok(header_str) = header_value.to_str() {
                    for part in header_str.split(';') {
                        let part = part.trim();
                        if let Some(ip_raw) = part.strip_prefix("for=") {
                            let ip = ip_raw.trim_matches('"').trim();
                            if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                                return Ok(ip.to_string());
                            }
                        }
                    }
                }
            }

            // X-Real-IP
            if let Some(header_value) = req.headers().get("X-Real-IP") {
                if let Ok(ip) = header_value.to_str() {
                    let ip = ip.trim();
                    if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                        return Ok(ip.to_string());
                    }
                }
            }
        }

        // Use peer address for direct (non-proxied) connections or when forwarded header is absent.
        if let Some(ip) = peer_ip {
            return Ok(ip.to_string());
        }

        Err(GovernorError::UnableToExtractKey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, Method, Uri, Version};

    fn create_request_with_header(name: &str, value: &str) -> Request<()> {
        use axum::http::HeaderName;
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );

        let mut req = Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .version(Version::HTTP_11)
            .body(())
            .unwrap();
        *req.headers_mut() = headers;
        req
    }

    #[test]
    fn test_x_forwarded_for_extraction() {
        let extractor = ProxyAwareKeyExtractor;
        let req = create_request_with_header("X-Forwarded-For", "192.168.1.1");
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "192.168.1.1");
    }

    #[test]
    fn test_x_forwarded_for_multiple_ips() {
        let extractor = ProxyAwareKeyExtractor;
        let req = create_request_with_header("X-Forwarded-For", "192.168.1.1, 10.0.0.1");
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "192.168.1.1");
    }

    #[test]
    fn test_forwarded_header_extraction() {
        let extractor = ProxyAwareKeyExtractor;
        let req = create_request_with_header("Forwarded", "for=192.168.1.1;proto=https");
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "192.168.1.1");
    }

    #[test]
    fn test_x_real_ip_extraction() {
        let extractor = ProxyAwareKeyExtractor;
        let req = create_request_with_header("X-Real-IP", "192.168.1.1");
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "192.168.1.1");
    }

    #[test]
    fn test_priority_x_forwarded_for_over_forwarded() {
        let extractor = ProxyAwareKeyExtractor;
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_str("192.168.1.1").unwrap(),
        );
        headers.insert("Forwarded", HeaderValue::from_str("for=10.0.0.1").unwrap());

        let mut req = Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .body(())
            .unwrap();
        *req.headers_mut() = headers;

        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "192.168.1.1");
    }
}
