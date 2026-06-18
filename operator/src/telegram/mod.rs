//! Telegram signal ingestion module
//!
//! This module treats Telegram channels as virtual wallet signal sources in the
//! Chimera roster system. Each channel has its own WQS score, tracked performance,
//! and quality metrics, allowing them to participate in consensus detection and
//! signal quality scoring alongside on-chain wallets.
//!
//! # Architecture
//!
//! ```text
//! Telegram Channels → Python Collector → Virtual Wallet System →
//! Signal Quality Scoring → Consensus Detection → Standard Execution
//! ```
//!
//! # Components
//!
//! - [`client`]: Telegram client wrapper for API communication
//! - [`parser`]: Signal parsing logic for extracting trading data from messages
//! - [`source_manager`]: Channel management and signal processing

pub mod client;
pub mod parser;
pub mod source_manager;

pub use source_manager::{ChannelConfig, TelegramSourceManager};

// Re-export common types
pub use parser::{ParsedTelegramSignal, SignalConfidence};

// Error type for telegram module
pub use telegram_error::{TelegramError, TelegramResult};

mod telegram_error {
    use std::fmt;

    /// Error type for Telegram signal operations
    #[derive(Debug)]
    pub enum TelegramError {
        /// Parse error - signal could not be parsed
        ParseError(String),
        /// Invalid signal - signal validation failed
        InvalidSignal(String),
        /// Rate limit exceeded for channel
        RateLimitExceeded(String),
        /// Channel not found or not configured
        ChannelNotFound(String),
        /// Database operation failed
        DatabaseError(String),
        /// API communication error
        ApiError(String),
    }

    impl fmt::Display for TelegramError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                TelegramError::ParseError(msg) => write!(f, "Parse error: {}", msg),
                TelegramError::InvalidSignal(msg) => write!(f, "Invalid signal: {}", msg),
                TelegramError::RateLimitExceeded(channel) => {
                    write!(f, "Rate limit exceeded for channel: {}", channel)
                }
                TelegramError::ChannelNotFound(channel) => {
                    write!(f, "Channel not found: {}", channel)
                }
                TelegramError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
                TelegramError::ApiError(msg) => write!(f, "API error: {}", msg),
            }
        }
    }

    impl std::error::Error for TelegramError {}

    /// Result type for Telegram operations
    pub type TelegramResult<T> = Result<T, TelegramError>;
}
