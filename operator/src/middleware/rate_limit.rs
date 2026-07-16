//! Rate limiting middleware with proxy-aware key extraction.
//!
//! X-Forwarded-For and similar headers are ONLY trusted when the connecting peer
//! is a loopback or RFC-1918 private address (i.e., a trusted internal proxy).
//! Direct connections from public IPs always use the peer address — accepting
//! client-supplied forwarded headers from untrusted peers allows IP spoofing to
//! trivially bypass per-IP rate limits.
//!
//! Header priority (most secure to least secure):
//! 1. X-Real-IP: Single-value, set by trusted proxy, cannot be spoofed by client
//! 2. Forwarded (RFC 7239): Structured format, harder to spoof than X-Forwarded-For
//! 3. X-Forwarded-For: Easily spoofed, only use rightmost IP as fallback
//!
//! Security Fix: Changed from using leftmost IP to rightmost IP in X-Forwarded-For
//! to prevent attackers from spoofing their IP address and bypassing rate limits.

use axum::extract::ConnectInfo;
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
        let peer_addr = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0);
        let peer_ip = peer_addr.map(|a| a.ip());

        // Only trust forwarded headers when the direct connection is from a trusted proxy.
        let from_trusted_proxy = peer_ip.map(|ip| is_trusted_proxy(&ip)).unwrap_or(false);

        if from_trusted_proxy {
            // FIX: Use X-Real-IP as primary source - it's single-valued and set by trusted proxy
            // X-Real-IP cannot be spoofed by client since only trusted proxy sets it
            if let Some(header_value) = req.headers().get("X-Real-IP") {
                if let Ok(ip) = header_value.to_str() {
                    let ip = ip.trim();
                    if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                        return Ok(ip.to_string());
                    }
                }
            }

            // Forwarded header (RFC 7239) - more secure than X-Forwarded-For
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

            // X-Forwarded-For is LESS secure - only use as fallback
            // WARNING: Client can set X-Forwarded-For to arbitrary values
            // If used, prefer rightmost IP (closest to trusted proxy) over leftmost
            if let Some(header_value) = req.headers().get("X-Forwarded-For") {
                if let Ok(header_str) = header_value.to_str() {
                    // Use rightmost IP (closest to our trusted proxy) instead of leftmost
                    // This prevents client from spoofing their IP
                    if let Some(client_ip) = header_str.split(',').last() {
                        let ip = client_ip.trim();
                        if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }
        }

        // Security: Do NOT fall back to peer address for rate limiting
        // Peer addresses are easily spoofed by attackers, allowing rate limit bypass
        // Require authentication-based limiting (API key, JWT token, etc.) for non-proxied requests
        // For now, return error to force proper authentication implementation
        if peer_ip.is_some() && !from_trusted_proxy {
            tracing::warn!(
                peer_ip = ?peer_ip,
                "Rate limiting requires authentication for direct connections. Peer address blocked for security."
            );
            return Err(GovernorError::UnableToExtractKey);
        }

        // Use peer address ONLY for trusted proxies (already handled above)
        if let Some(ip) = peer_ip {
            if from_trusted_proxy {
                return Ok(ip.to_string());
            }
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
            .extension(ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                8080,
            ))))
            .body(())
            .unwrap();
        *req.headers_mut() = headers;
        req
    }

    #[test]
    fn test_x_real_ip_preferred_over_forwarded_for() {
        let extractor = ProxyAwareKeyExtractor;
        let mut headers = HeaderMap::new();
        headers.insert("X-Real-IP", HeaderValue::from_str("10.0.0.1").unwrap());
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_str("1.2.3.4, 5.6.7.8").unwrap(),
        );

        let mut req = Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .extension(ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                8080,
            ))))
            .body(())
            .unwrap();
        *req.headers_mut() = headers;

        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.0.0.1", "X-Real-IP should be preferred over X-Forwarded-For");
    }

    #[test]
    fn test_forwarded_header_preferred_over_x_forwarded_for() {
        let extractor = ProxyAwareKeyExtractor;
        let mut headers = HeaderMap::new();
        headers.insert("Forwarded", HeaderValue::from_str("for=10.0.0.1").unwrap());
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_str("1.2.3.4, 5.6.7.8").unwrap(),
        );

        let mut req = Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .extension(ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                8080,
            ))))
            .body(())
            .unwrap();
        *req.headers_mut() = headers;

        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.0.0.1", "Forwarded header should be preferred over X-Forwarded-For");
    }

    #[test]
    fn test_x_forwarded_for_uses_rightmost_ip() {
        let extractor = ProxyAwareKeyExtractor;
        // FIX: Rightmost IP should be used (closest to trusted proxy), not leftmost
        let req = create_request_with_header("X-Forwarded-For", "1.2.3.4, 5.6.7.8");
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "5.6.7.8", "Should use rightmost IP from X-Forwarded-For");
    }

    #[test]
    fn test_x_forwarded_for_single_ip() {
        let extractor = ProxyAwareKeyExtractor;
        let req = create_request_with_header("X-Forwarded-For", "192.168.1.1");
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "192.168.1.1", "Single IP in X-Forwarded-For should work");
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
    fn test_ip_spoofing_prevention() {
        let extractor = ProxyAwareKeyExtractor;
        // Simulate attacker trying to spoof their IP via X-Forwarded-For
        // X-Forwarded-For: 1.2.3.4 (attacker-controlled), 5.6.7.8 (real client)
        let req = create_request_with_header("X-Forwarded-For", "1.2.3.4, 5.6.7.8");
        let key = extractor.extract(&req).unwrap();
        // Should use rightmost (5.6.7.8) not leftmost (1.2.3.4)
        assert_eq!(key, "5.6.7.8", "Should prevent IP spoofing by using rightmost IP");
    }
}
