//! Telegram client wrapper
//!
//! Handles communication with Telegram for signal ingestion.
//! This is a lightweight wrapper - the actual Telegram monitoring
//! is done by the Python collector service.

use serde::{Deserialize, Serialize};

/// Telegram message received from the Python collector
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramMessage {
    /// Channel username (e.g., "@solana_whales_signal")
    pub channel: String,
    /// Numeric channel ID
    pub channel_id: i64,
    /// Message ID
    pub message_id: i32,
    /// Unix timestamp
    pub timestamp: i64,
    /// Message text content
    pub text: String,
    /// Optional media URLs
    pub media_urls: Vec<String>,
}

impl TelegramMessage {
    /// Convert to RawTelegramSignal for parsing
    pub fn to_raw_signal(self) -> super::parser::RawTelegramSignal {
        super::parser::RawTelegramSignal {
            channel: self.channel,
            channel_id: self.channel_id,
            message_id: self.message_id,
            timestamp: self.timestamp,
            text: self.text,
        }
    }
}

/// API payload for receiving Telegram signals from the Python collector
#[derive(Debug, Deserialize)]
pub struct TelegramSignalPayload {
    /// Telegram message
    pub message: TelegramMessage,
    /// API authentication token (if using internal API)
    pub auth_token: Option<String>,
}

/// Response for Telegram signal ingestion
#[derive(Debug, Serialize)]
pub struct TelegramSignalResponse {
    pub success: bool,
    pub message: String,
    pub signal_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_to_raw_signal() {
        let msg = TelegramMessage {
            channel: "@test".to_string(),
            channel_id: 12345,
            message_id: 67890,
            timestamp: 1640000000,
            text: "Buy $TOKEN now!".to_string(),
            media_urls: vec![],
        };

        let raw = msg.to_raw_signal();
        assert_eq!(raw.channel, "@test");
        assert_eq!(raw.text, "Buy $TOKEN now!");
    }
}
