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

/// Returns true when the IP is a loopback, RFC-1918 private, unique-local,
/// link-local, or IPv4-mapped private address (i.e., a trusted internal proxy).
fn is_trusted_proxy(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| v4.is_loopback() || v4.is_private())
                    .unwrap_or(false)
        }
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
                    if ip.parse::<IpAddr>().is_ok() {
                        return Ok(ip.to_string());
                    }
                }
            }

            // Forwarded header (RFC 7239) - more secure than X-Forwarded-For.
            // The last comma-separated hop is the one added by our trusted proxy;
            // parse the `for=` parameter there (brackets/ports for IPv6 allowed).
            if let Some(header_value) = req.headers().get("Forwarded") {
                if let Ok(header_str) = header_value.to_str() {
                    if let Some(last_hop) = header_str.split(',').next_back() {
                        for part in last_hop.split(';') {
                            let part = part.trim();
                            if let Some(ip_raw) = part.strip_prefix("for=") {
                                let ip = ip_raw
                                    .trim_matches('"')
                                    .trim()
                                    .trim_start_matches('[')
                                    .trim_end_matches(']');
                                if let Ok(addr) = ip.parse::<SocketAddr>() {
                                    return Ok(addr.ip().to_string());
                                }
                                if ip.parse::<IpAddr>().is_ok() {
                                    return Ok(ip.to_string());
                                }
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
                    if let Some(client_ip) = header_str.split(',').next_back() {
                        let ip = client_ip.trim();
                        if ip.parse::<IpAddr>().is_ok() {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }
        }

        // Fall back to the peer address as the rate-limit key. The TCP peer
        // socket address cannot be spoofed by a client (unlike a header), so
        // per-peer limiting remains meaningful for direct connections.
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

    // ==========================================================================
    // is_trusted_proxy
    // ==========================================================================

    #[test]
    fn test_is_trusted_proxy_v4() {
        assert!(is_trusted_proxy(&"127.0.0.1".parse().unwrap()));
        assert!(is_trusted_proxy(&"10.0.0.1".parse().unwrap()));
        assert!(is_trusted_proxy(&"192.168.1.1".parse().unwrap()));
        assert!(is_trusted_proxy(&"172.16.0.1".parse().unwrap()));
        assert!(!is_trusted_proxy(&"8.8.8.8".parse().unwrap()));
        assert!(!is_trusted_proxy(&"1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn test_is_trusted_proxy_v6() {
        // Loopback
        assert!(is_trusted_proxy(&"::1".parse().unwrap()));
        // Unique-local (fc00::/7)
        assert!(is_trusted_proxy(&"fd00::1".parse().unwrap()));
        assert!(is_trusted_proxy(&"fc00::1".parse().unwrap()));
        // Link-local (fe80::/10)
        assert!(is_trusted_proxy(&"fe80::1".parse().unwrap()));
        assert!(is_trusted_proxy(&"febf::1".parse().unwrap()));
        // IPv4-mapped loopback and private
        assert!(is_trusted_proxy(&"::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_trusted_proxy(&"::ffff:10.1.2.3".parse().unwrap()));
        // Global unicast
        assert!(!is_trusted_proxy(&"2606:4700:4700::1111".parse().unwrap()));
        assert!(!is_trusted_proxy(&"2001:4860:4860::8888".parse().unwrap()));
        // fec0::/10 (site-local) is NOT treated as trusted
        assert!(!is_trusted_proxy(&"fec0::1".parse().unwrap()));
    }

    // ==========================================================================
    // Key extraction paths
    // ==========================================================================

    fn request_from(peer: [u16; 8], port: u16) -> Request<()> {
        use axum::http::{Method, Uri, Version};
        Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .version(Version::HTTP_11)
            .extension(ConnectInfo(std::net::SocketAddr::new(
                std::net::IpAddr::V6(std::net::Ipv6Addr::from(peer)),
                port,
            )))
            .body(())
            .unwrap()
    }

    fn request_from_v4(octets: [u8; 4], port: u16) -> Request<()> {
        use axum::http::{Method, Uri, Version};
        Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .version(Version::HTTP_11)
            .extension(ConnectInfo(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::from(octets)),
                port,
            )))
            .body(())
            .unwrap()
    }

    fn with_header(mut req: Request<()>, name: &str, value: &str) -> Request<()> {
        use axum::http::HeaderName;
        req.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        req
    }

    #[test]
    fn test_untrusted_peer_ignores_forwarded_headers() {
        let extractor = ProxyAwareKeyExtractor;
        // Public peer (8.8.8.8) supplying X-Forwarded-For must NOT be honored —
        // the peer address is the only trustworthy key.
        let req = with_header(
            request_from_v4([8, 8, 8, 8], 443),
            "X-Forwarded-For",
            "1.2.3.4",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "8.8.8.8");
    }

    #[test]
    fn test_untrusted_peer_ignores_x_real_ip() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([8, 8, 8, 8], 443),
            "X-Real-IP",
            "10.0.0.9",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "8.8.8.8");
    }

    #[test]
    fn test_no_connect_info_errors() {
        let extractor = ProxyAwareKeyExtractor;
        let req = Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .body(())
            .unwrap();
        assert!(matches!(
            extractor.extract(&req),
            Err(GovernorError::UnableToExtractKey)
        ));
    }

    #[test]
    fn test_forwarded_header_socket_addr_ipv6() {
        let extractor = ProxyAwareKeyExtractor;
        // RFC 7239 form with IPv6 in brackets (no port) — brackets are trimmed
        let req = with_header(
            create_request_with_header("Forwarded", "for=\"[2001:db8::1]\""),
            "Forwarded",
            "for=\"[2001:db8::1]\"",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "2001:db8::1");
    }

    #[test]
    fn test_forwarded_header_socket_addr_with_port() {
        let extractor = ProxyAwareKeyExtractor;
        // Unbracketed host:port parses as a SocketAddr, extracting the IP
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "Forwarded",
            "for=10.0.0.5:4711",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.0.0.5");
    }

    #[test]
    fn test_forwarded_header_ipv6_brackets_no_port() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "Forwarded",
            "for=[2001:db8::1]",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "2001:db8::1");
    }

    #[test]
    fn test_forwarded_header_quoted_simple_ip() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "Forwarded",
            "for=\"10.1.1.1\"",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.1.1.1");
    }

    #[test]
    fn test_forwarded_header_multiple_hops_uses_last() {
        let extractor = ProxyAwareKeyExtractor;
        // Comma-separated hops; the last one is added by our trusted proxy.
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "Forwarded",
            "for=1.2.3.4;proto=http, for=10.2.3.4;proto=https",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.2.3.4");
    }

    #[test]
    fn test_forwarded_header_invalid_for_falls_back() {
        let extractor = ProxyAwareKeyExtractor;
        // Invalid `for=` value: both SocketAddr and IpAddr parses fail → peer key.
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "Forwarded",
            "for=not-an-ip;proto=https",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "127.0.0.1");
    }

    #[test]
    fn test_forwarded_header_no_for_part_falls_back() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "Forwarded",
            "proto=https;host=example.com",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "127.0.0.1");
    }

    #[test]
    fn test_x_real_ip_invalid_falls_back_to_forwarded() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            with_header(
                request_from_v4([127, 0, 0, 1], 8080),
                "X-Real-IP",
                "definitely-not-an-ip",
            ),
            "Forwarded",
            "for=10.9.9.9",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.9.9.9");
    }

    #[test]
    fn test_x_forwarded_for_invalid_ip_falls_back_to_peer() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "X-Forwarded-For",
            "garbage, also-garbage",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "127.0.0.1");
    }

    #[test]
    fn test_x_forwarded_for_whitespace_trimmed() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "X-Forwarded-For",
            "1.2.3.4,  5.6.7.8  ",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "5.6.7.8");
    }

    #[test]
    fn test_x_real_ip_whitespace_trimmed() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from_v4([127, 0, 0, 1], 8080),
            "X-Real-IP",
            " 10.1.1.1 ",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "10.1.1.1");
    }

    #[test]
    fn test_ipv6_trusted_proxy_with_forwarded_headers() {
        let extractor = ProxyAwareKeyExtractor;
        // IPv6 loopback peer counts as a trusted proxy
        let req = with_header(
            request_from([0, 0, 0, 0, 0, 0, 0, 1], 8080),
            "X-Forwarded-For",
            "1.2.3.4, 5.6.7.8",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "5.6.7.8");
    }

    #[test]
    fn test_ipv6_untrusted_peer_uses_peer_address() {
        let extractor = ProxyAwareKeyExtractor;
        let req = with_header(
            request_from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111], 443),
            "X-Forwarded-For",
            "1.2.3.4",
        );
        let key = extractor.extract(&req).unwrap();
        assert_eq!(key, "2606:4700:4700::1111");
    }
}
