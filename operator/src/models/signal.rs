//! Signal models - represents incoming webhook signals

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Trading strategy types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Strategy {
    /// Conservative strategy - lower risk, lower reward
    Shield,
    /// Aggressive strategy - higher risk, higher reward
    Spear,
    /// Exit signal - close position
    Exit,
}

impl Strategy {
    /// Get priority for queue ordering (lower = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            Strategy::Exit => 0,   // Highest priority - protect capital
            Strategy::Shield => 1, // Second priority - conservative trades
            Strategy::Spear => 2,  // Lowest priority - aggressive trades
        }
    }

    /// Check if this strategy should be shed during load shedding
    pub fn is_sheddable(&self) -> bool {
        matches!(self, Strategy::Spear)
    }
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Strategy::Shield => write!(f, "SHIELD"),
            Strategy::Spear => write!(f, "SPEAR"),
            Strategy::Exit => write!(f, "EXIT"),
        }
    }
}

/// Trade action (buy or sell)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Action {
    Buy,
    Sell,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::Buy => write!(f, "BUY"),
            Action::Sell => write!(f, "SELL"),
        }
    }
}

/// Incoming webhook signal payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalPayload {
    /// Trading strategy
    pub strategy: Strategy,
    /// Token symbol (e.g., "BONK")
    pub token: String,
    /// Token mint address (Solana pubkey)
    #[serde(default)]
    pub token_address: Option<String>,
    /// Trade action
    pub action: Action,
    /// Amount in SOL
    pub amount_sol: Decimal,
    /// Wallet address being copied
    pub wallet_address: String,
    /// Optional trade UUID from signal provider
    #[serde(default)]
    pub trade_uuid: Option<String>,
}

impl SignalPayload {
    /// Generate a deterministic trade UUID if not provided
    ///
    /// Uses SHA256(timestamp + token + action + amount) for idempotency
    pub fn generate_trade_uuid(&self, timestamp: i64) -> String {
        if let Some(ref uuid) = self.trade_uuid {
            return uuid.clone();
        }

        let mut hasher = Sha256::new();
        hasher.update(timestamp.to_be_bytes());
        hasher.update(self.token.as_bytes());
        hasher.update(self.action.to_string().as_bytes());
        // Convert Decimal to bytes for hashing (use to_string to ensure consistent representation)
        let amount_str = self.amount_sol.to_string();
        hasher.update(amount_str.as_bytes());
        hasher.update(self.wallet_address.as_bytes());

        let result = hasher.finalize();
        hex::encode(&result[..16]) // Use first 16 bytes for shorter UUID
    }

    /// Validate the signal payload
    pub fn validate(&self) -> Result<(), String> {
        // Check token is not empty
        if self.token.trim().is_empty() {
            return Err("Token symbol cannot be empty".to_string());
        }

        // Check wallet address looks valid (basic check)
        if self.wallet_address.len() < 32 || self.wallet_address.len() > 44 {
            return Err("Invalid wallet address length".to_string());
        }

        // Check amount is positive and reasonable
        if self.amount_sol <= Decimal::ZERO {
            return Err("Amount must be positive".to_string());
        }

        if self.amount_sol > Decimal::from(100) {
            return Err("Amount exceeds maximum (100 SOL)".to_string());
        }

        // Exit signals must be SELL
        if self.strategy == Strategy::Exit && self.action != Action::Sell {
            return Err("Exit strategy must have SELL action".to_string());
        }

        Ok(())
    }
}

/// Parsed and validated signal ready for processing
#[derive(Debug, Clone)]
pub struct Signal {
    /// Unique trade identifier
    pub trade_uuid: String,
    /// Original payload
    pub payload: SignalPayload,
    /// Unix timestamp from request
    pub timestamp: i64,
    /// Source IP address
    pub source_ip: Option<String>,
}

impl Signal {
    /// Create a new signal from validated payload
    pub fn new(payload: SignalPayload, timestamp: i64, source_ip: Option<String>) -> Self {
        let trade_uuid = payload.generate_trade_uuid(timestamp);
        Self {
            trade_uuid,
            payload,
            timestamp,
            source_ip,
        }
    }

    /// Get the token address, falling back to symbol if not provided
    pub fn token_address(&self) -> &str {
        self.payload
            .token_address
            .as_deref()
            .unwrap_or(&self.payload.token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_priority() {
        assert!(Strategy::Exit.priority() < Strategy::Shield.priority());
        assert!(Strategy::Shield.priority() < Strategy::Spear.priority());
    }

    #[test]
    fn test_signal_validation() {
        let valid_signal = SignalPayload {
            strategy: Strategy::Shield,
            token: "BONK".to_string(),
            token_address: None,
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
        };

        assert!(valid_signal.validate().is_ok());
    }

    #[test]
    fn test_trade_uuid_generation() {
        let signal = SignalPayload {
            strategy: Strategy::Shield,
            token: "BONK".to_string(),
            token_address: None,
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
        };

        let uuid1 = signal.generate_trade_uuid(1234567890);
        let uuid2 = signal.generate_trade_uuid(1234567890);

        // Same inputs should generate same UUID (deterministic)
        assert_eq!(uuid1, uuid2);

        // Different timestamp should generate different UUID
        let uuid3 = signal.generate_trade_uuid(1234567891);
        assert_ne!(uuid1, uuid3);
    }

    #[test]
    fn test_provided_uuid_preserved() {
        let signal = SignalPayload {
            strategy: Strategy::Shield,
            token: "BONK".to_string(),
            token_address: None,
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: Some("custom-uuid-123".to_string()),
        };

        assert_eq!(signal.generate_trade_uuid(0), "custom-uuid-123");
    }
}
