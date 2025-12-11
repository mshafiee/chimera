//! Rate limiting middleware with custom key extractor for proxy support
//!
//! Extracts client IP from X-Forwarded-For or Forwarded headers,
//! falling back to peer address for direct connections.

use axum::http::Request;
use std::net::SocketAddr;
use tower_governor::{key_extractor::KeyExtractor, GovernorError};

/// Custom key extractor that prefers forwarded headers, falls back to peer address
#[derive(Clone)]
pub struct ProxyAwareKeyExtractor;

impl KeyExtractor for ProxyAwareKeyExtractor {
    type Key = String;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        // Try X-Forwarded-For first (most common)
        if let Some(header_value) = req.headers().get("X-Forwarded-For") {
            if let Ok(header_str) = header_value.to_str() {
                // X-Forwarded-For can contain multiple IPs (comma-separated)
                // The first one is typically the original client IP
                if let Some(client_ip) = header_str.split(',').next() {
                    let ip = client_ip.trim();
                    // Basic validation: check if it looks like an IP
                    if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                        return Ok(ip.to_string());
                    }
                }
            }
        }

        // Try Forwarded header (RFC 7239)
        if let Some(header_value) = req.headers().get("Forwarded") {
            if let Ok(header_str) = header_value.to_str() {
                // Forwarded header format: "for=192.0.2.60;proto=http;by=203.0.113.43"
                // Extract the first "for=" value
                for part in header_str.split(';') {
                    let part = part.trim();
                    if part.starts_with("for=") {
                        let ip = part[4..].trim_matches('"').trim();
                        if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }
        }

        // Try X-Real-IP (some proxies use this)
        if let Some(header_value) = req.headers().get("X-Real-IP") {
            if let Ok(ip) = header_value.to_str() {
                let ip = ip.trim();
                if !ip.is_empty() && (ip.contains('.') || ip.contains(':')) {
                    return Ok(ip.to_string());
                }
            }
        }

        // Fallback to peer address from extensions
        if let Some(peer_addr) = req.extensions().get::<SocketAddr>() {
            return Ok(peer_addr.ip().to_string());
        }

        // If we can't extract any IP, use a default key
        // This should rarely happen, but we need to return something
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
        headers.insert("X-Forwarded-For", HeaderValue::from_str("192.168.1.1").unwrap());
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
