//! Telegram signal parser
//!
//! Extracts trading signals from Telegram messages including token addresses,
//! symbols, confidence levels, and other metadata.

use super::telegram_error::{TelegramError, TelegramResult};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Confidence level of a trading signal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalConfidence {
    High,
    Medium,
    Low,
    Unknown,
}

impl Default for SignalConfidence {
    fn default() -> Self {
        SignalConfidence::Unknown
    }
}

impl SignalConfidence {
    /// Convert confidence to numeric score (0-1)
    pub fn to_score(&self) -> f64 {
        match self {
            SignalConfidence::High => 1.0,
            SignalConfidence::Medium => 0.6,
            SignalConfidence::Low => 0.3,
            SignalConfidence::Unknown => 0.5,
        }
    }

    /// Parse confidence from text/emojis
    pub fn from_text(text: &str) -> Self {
        let text_lower = text.to_lowercase();

        // Check for explicit confidence indicators
        if text_lower.contains("strong buy")
            || text_lower.contains("high confidence")
            || text_lower.contains("sure")
        {
            return SignalConfidence::High;
        }

        if text_lower.contains("speculative")
            || text_lower.contains("risky")
            || text_lower.contains("low confidence")
        {
            return SignalConfidence::Low;
        }

        // Check emoji indicators
        let high_emojis = ["🚀", "💎", "🔥", "⭐", "✅", "💯"];
        let medium_emojis = ["👀", "📈", "🔔", "⚡", "💰"];
        let low_emojis = ["⚠️", "🤡", "📉", "❌", "💀"];

        let high_count = high_emojis.iter().filter(|&&e| text.contains(e)).count();
        let medium_count = medium_emojis.iter().filter(|&&e| text.contains(e)).count();
        let low_count = low_emojis.iter().filter(|&&e| text.contains(e)).count();

        if high_count >= 2 || (high_count >= 1 && low_count == 0) {
            SignalConfidence::High
        } else if low_count >= 2 {
            SignalConfidence::Low
        } else if medium_count >= 1 || high_count == 1 {
            SignalConfidence::Medium
        } else {
            SignalConfidence::Unknown
        }
    }
}

/// Parsed Telegram trading signal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedTelegramSignal {
    /// Channel username (e.g., "@solana_whales_signal")
    pub channel: String,
    /// Channel ID (numeric)
    pub channel_id: i64,
    /// Message ID
    pub message_id: i32,
    /// Unix timestamp of message
    pub timestamp: i64,
    /// Token contract address
    pub token_address: String,
    /// Token symbol/ticker
    pub token_symbol: Option<String>,
    /// Signal confidence level
    pub confidence: SignalConfidence,
    /// Whether message includes chart link
    pub has_chart: bool,
    /// Whether message includes NFA disclaimer
    pub has_caution: bool,
    /// Raw message text (truncated)
    pub raw_text: String,
}

/// Raw Telegram message from the Python collector
#[derive(Debug, Clone, Deserialize)]
pub struct RawTelegramSignal {
    pub channel: String,
    pub channel_id: i64,
    pub message_id: i32,
    pub timestamp: i64,
    pub text: String,
}

/// Telegram signal parser
pub struct TelegramParser {
    /// Solana token address pattern (base58, 32-44 chars)
    token_address_pattern: Regex,
    /// Token symbol pattern ($SYMBOL)
    token_symbol_pattern: Regex,
}

impl TelegramParser {
    /// Create a new Telegram parser
    pub fn new() -> Self {
        Self {
            token_address_pattern: Regex::new(r"\b[1-9A-HJ-NP-Za-km-z]{32,44}\b").unwrap(),
            token_symbol_pattern: Regex::new(r"\$([A-Z]{1,10})\b").unwrap(),
        }
    }

    /// Check if message contains trading signal indicators
    pub fn is_signal_message(&self, text: &str) -> bool {
        let text_lower = text.to_lowercase();
        let keywords = [
            "pump", "buy", "entry", "target", "launch", "new token", "gem",
            "call", "moon", "rocket", "alpha", "contract", "address",
            "ca:", "contract:", "token:", "symbol:", "ticker:", "$",
        ];

        keywords.iter().any(|k| text_lower.contains(k))
    }

    /// Extract token address from text
    fn extract_token_address(&self, text: &str) -> TelegramResult<String> {
        // Look for patterns like "ca:", "contract:", "address:"
        for prefix in ["ca:", "contract:", "address:", "token:"] {
            let pattern = Regex::new(&format!(r"{}\s*([1-9A-HJ-NP-Za-km-z]{{32,44}})", regex::escape(prefix))).unwrap();
            if let Some(m) = pattern.find(text) {
                let addr = text[m.start() + prefix.len()..m.end()].trim();
                return Ok(addr.to_string());
            }
        }

        // Fallback: find first valid Solana address
        if let Some(m) = self.token_address_pattern.find(text) {
            return Ok(text[m.start()..m.end()].to_string());
        }

        Err(TelegramError::ParseError("No token address found".to_string()))
    }

    /// Extract token symbol from text
    fn extract_token_symbol(&self, text: &str) -> Option<String> {
        // Look for $SYMBOL patterns
        if let Some(caps) = self.token_symbol_pattern.captures(text) {
            if let Some(symbol) = caps.get(1) {
                return Some(symbol.as_str().to_string());
            }
        }

        // Look for "symbol:" or "ticker:" patterns
        for prefix in ["symbol:", "ticker:", "name:"] {
            let pattern = Regex::new(&format!(r"{}\s*([A-Z]{{1,10}})\b", regex::escape(prefix))).unwrap();
            if let Some(caps) = pattern.captures(text) {
                if let Some(symbol) = caps.get(1) {
                    return Some(symbol.as_str().to_string());
                }
            }
        }

        None
    }

    /// Check if message has chart link
    fn has_chart_link(&self, text: &str) -> bool {
        let chart_domains = [
            "dexscreener.com", "dexscreener",
            "photon.sol", "birdeye.so", "birdeye",
            "bullx", "geckoterminal",
        ];
        chart_domains.iter().any(|domain| text.to_lowercase().contains(domain))
    }

    /// Check if message has NFA/disclaimer
    fn has_caution(&self, text: &str) -> bool {
        let caution_keywords = [
            "nfa", "not financial advice", "do your own research",
            "dyor", "risk", "not advice",
        ];
        let text_lower = text.to_lowercase();
        caution_keywords.iter().any(|k| text_lower.contains(k))
    }

    /// Parse raw Telegram signal into structured format
    pub fn parse(&self, raw: RawTelegramSignal) -> TelegramResult<ParsedTelegramSignal> {
        // Validate this looks like a signal
        if !self.is_signal_message(&raw.text) {
            return Err(TelegramError::ParseError("Message does not contain signal indicators".to_string()));
        }

        // Extract token address (required)
        let token_address = self.extract_token_address(&raw.text)?;

        // Extract optional fields
        let token_symbol = self.extract_token_symbol(&raw.text);
        let confidence = SignalConfidence::from_text(&raw.text);
        let has_chart = self.has_chart_link(&raw.text);
        let has_caution = self.has_caution(&raw.text);

        // Truncate raw text for storage
        let raw_text = if raw.text.len() > 500 {
            format!("{}...", &raw.text[..500])
        } else {
            raw.text.clone()
        };

        Ok(ParsedTelegramSignal {
            channel: raw.channel,
            channel_id: raw.channel_id,
            message_id: raw.message_id,
            timestamp: raw.timestamp,
            token_address,
            token_symbol,
            confidence,
            has_chart,
            has_caution,
            raw_text,
        })
    }
}

impl Default for TelegramParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_address_extraction() {
        let parser = TelegramParser::new();
        let text = "Buy now! ca: 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
        let addr = parser.extract_token_address(text).unwrap();
        assert_eq!(addr, "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU");
    }

    #[test]
    fn test_token_symbol_extraction() {
        let parser = TelegramParser::new();
        let text = "Token $BONK is going to moon!";
        let symbol = parser.extract_token_symbol(text);
        assert_eq!(symbol, Some("BONK".to_string()));
    }

    #[test]
    fn test_confidence_detection() {
        assert!(matches!(SignalConfidence::from_text("STRONG BUY NOW! 🚀🚀"), SignalConfidence::High));
        assert!(matches!(SignalConfidence::from_text("Speculative play 👀"), SignalConfidence::Medium));
        assert!(matches!(SignalConfidence::from_text("Risky, not financial advice ⚠️"), SignalConfidence::Low));
    }

    #[test]
    fn test_chart_detection() {
        let parser = TelegramParser::new();
        assert!(parser.has_chart_link("Check dexscreener.com/solana/0x..."));
        assert!(!parser.has_chart_link("No link here"));
    }
}
