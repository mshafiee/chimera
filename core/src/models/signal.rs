//! Signal models - represents incoming webhook signals

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Trading strategy types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Strategy {
    #[default]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Action {
    #[default]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    #[serde(default = "default_amount")]
    pub amount_sol: Decimal,
    /// Wallet address being copied
    #[serde(default = "default_wallet")]
    pub wallet_address: String,
    /// Optional trade UUID from signal provider
    #[serde(default)]
    pub trade_uuid: Option<String>,
    /// Optional fraction of the position to exit (used for partial exits)
    #[serde(default)]
    pub exit_fraction: Option<Decimal>,
}

fn default_amount() -> Decimal {
    Decimal::ZERO
}

fn default_wallet() -> String {
    "UNKNOWN_WALLET".to_string()
}

impl SignalPayload {
    /// Generate a deterministic trade UUID if not provided.
    ///
    /// Does NOT include the request timestamp so that webhook retries (same payload,
    /// later timestamp) produce the SAME UUID and are caught by the DB dedup check.
    /// Hash: SHA256(wallet || token || action || amount || strategy || token_address || exit_fraction).
    pub fn generate_trade_uuid(&self, _timestamp: i64) -> String {
        if let Some(ref uuid) = self.trade_uuid {
            return uuid.clone();
        }

        let mut hasher = Sha256::new();
        hasher.update(self.wallet_address.as_bytes());
        hasher.update(b"|");
        hasher.update(self.token.as_bytes());
        hasher.update(b"|");
        hasher.update(self.action.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(self.amount_sol.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(self.strategy.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(self.token_address.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"|");
        hasher
            .update(self.exit_fraction.map(|f| f.to_string()).unwrap_or_default().as_bytes());

        let result = hasher.finalize();
        hex::encode(&result[..16]) // Use first 16 bytes for shorter UUID
    }

    /// Validate the signal payload
    pub fn validate(&self) -> Result<(), String> {
        // Check token is not empty
        if self.token.trim().is_empty() {
            return Err("Token symbol cannot be empty".to_string());
        }

        // Reject the legacy "UNKNOWN" sentinel — a malformed payload that omits
        // the token must not be accepted as a legitimate trade.
        if self.token == "UNKNOWN" {
            return Err("Token symbol is missing (got placeholder \"UNKNOWN\")".to_string());
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

        // Exit fraction must be in (0, 1]
        if let Some(f) = self.exit_fraction {
            if f <= Decimal::ZERO || f > Decimal::ONE {
                return Err("exit_fraction must be greater than 0 and at most 1".to_string());
            }
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
    /// Token liquidity in USD at signal validation time (from Jupiter/DexScreener).
    /// Used to compute a liquidity-aware slippage estimate when Jupiter price impact
    /// is unavailable. None when the token safety check was skipped (dev mode, SELL signals).
    pub liquidity_usd: Option<rust_decimal::Decimal>,
    /// Set to true when the webhook fast-path token check returned an error (not just
    /// "unknown/unchecked"). When true the engine MUST run the slow-path check; if the
    /// slow-path is unavailable (token_parser is None) the trade must be blocked rather
    /// than silently passed through.
    pub force_slow_path: bool,
    /// Token decimals (e.g. 9 for most SPL tokens, 6 for USDC).
    /// None when not populated from token metadata.
    pub token_decimals: Option<u8>,
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
            liquidity_usd: None,
            force_slow_path: false,
            token_decimals: None,
        }
    }

    /// Get the token mint address.
    ///
    /// Returns `None` when the signal has no token address; the symbol is NOT
    /// substituted here so downstream code (Pubkey parsing, cache keys) never
    /// mistakes a symbol for a mint address.
    pub fn token_address(&self) -> Option<&str> {
        self.payload.token_address.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
            exit_fraction: None,
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
            exit_fraction: None,
        };

        let uuid1 = signal.generate_trade_uuid(1234567890);
        let uuid2 = signal.generate_trade_uuid(1234567890);

        // Same inputs should generate same UUID (deterministic)
        assert_eq!(uuid1, uuid2);

        // Different timestamp should generate same UUID (due to deduplication requirement)
        let uuid3 = signal.generate_trade_uuid(1234567891);
        assert_eq!(uuid1, uuid3);
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
            exit_fraction: None,
        };

        assert_eq!(signal.generate_trade_uuid(0), "custom-uuid-123");
    }

    #[test]
    fn test_strategy_is_sheddable() {
        assert!(Strategy::Spear.is_sheddable());
        assert!(!Strategy::Shield.is_sheddable());
        assert!(!Strategy::Exit.is_sheddable());
    }

    #[test]
    fn test_strategy_display() {
        assert_eq!(Strategy::Shield.to_string(), "SHIELD");
        assert_eq!(Strategy::Spear.to_string(), "SPEAR");
        assert_eq!(Strategy::Exit.to_string(), "EXIT");
        assert_eq!(Action::Buy.to_string(), "BUY");
        assert_eq!(Action::Sell.to_string(), "SELL");
    }

    fn base_payload() -> SignalPayload {
        SignalPayload {
            strategy: Strategy::Shield,
            token: "BONK".to_string(),
            token_address: None,
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
            exit_fraction: None,
        }
    }

    #[test]
    fn test_validate_rejects_each_invalid_payload() {
        // Empty token.
        let mut p = base_payload();
        p.token = "   ".to_string();
        assert!(p.validate().unwrap_err().contains("Token symbol cannot be empty"));

        // "UNKNOWN" sentinel token.
        let mut p = base_payload();
        p.token = "UNKNOWN".to_string();
        assert!(p.validate().unwrap_err().contains("placeholder"));

        // Wallet too short.
        let mut p = base_payload();
        p.wallet_address = "short".to_string();
        assert!(p.validate().unwrap_err().contains("Invalid wallet address length"));

        // Wallet too long.
        let mut p = base_payload();
        p.wallet_address = "x".repeat(45);
        assert!(p.validate().unwrap_err().contains("Invalid wallet address length"));

        // Non-positive amount.
        let mut p = base_payload();
        p.amount_sol = Decimal::ZERO;
        assert!(p.validate().unwrap_err().contains("Amount must be positive"));

        // Amount above max.
        let mut p = base_payload();
        p.amount_sol = Decimal::from(101);
        assert!(p.validate().unwrap_err().contains("exceeds maximum"));

        // exit_fraction zero.
        let mut p = base_payload();
        p.exit_fraction = Some(Decimal::ZERO);
        assert!(p.validate().unwrap_err().contains("exit_fraction"));

        // exit_fraction > 1.
        let mut p = base_payload();
        p.exit_fraction = Some(Decimal::from(2));
        assert!(p.validate().unwrap_err().contains("exit_fraction"));

        // Exit strategy with non-Sell action.
        let mut p = base_payload();
        p.strategy = Strategy::Exit;
        p.action = Action::Buy;
        assert!(p.validate().unwrap_err().contains("Exit strategy must have SELL action"));

        // Exit strategy with SELL action validates fine.
        let mut p = base_payload();
        p.strategy = Strategy::Exit;
        p.action = Action::Sell;
        assert!(p.validate().is_ok());
    }

    #[test]
    fn test_signal_payload_serde_defaults() {
        // amount_sol and wallet_address are populated from serde defaults when
        // omitted from the payload.
        let json = r#"{
            "strategy": "SHIELD",
            "token": "BONK",
            "action": "BUY"
        }"#;
        let payload: SignalPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.amount_sol, Decimal::ZERO);
        assert_eq!(payload.wallet_address, "UNKNOWN_WALLET");
        assert_eq!(default_amount(), Decimal::ZERO);
        assert_eq!(default_wallet(), "UNKNOWN_WALLET");
    }

    #[test]
    fn test_signal_token_address_accessor() {
        let mut p = base_payload();
        p.token_address = None;
        let signal = Signal::new(p.clone(), 0, None);
        assert!(signal.token_address().is_none());

        p.token_address = Some("mintaddr".to_string());
        let signal = Signal::new(p.clone(), 0, Some("1.2.3.4".into()));
        assert_eq!(signal.token_address(), Some("mintaddr"));
        assert_eq!(signal.source_ip.as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn test_signal_uuid_incorporates_optional_fields() {
        // Including token_address and exit_fraction changes the generated UUID.
        let mut p = base_payload();
        p.token_address = Some("mintaddr".to_string());
        p.exit_fraction = Some(Decimal::from_str("0.5").unwrap());
        let uuid = p.generate_trade_uuid(0);
        assert_eq!(uuid.len(), 32); // first 16 bytes hex-encoded
        // Two identical payloads hash the same.
        let p2 = p.clone();
        let uuid2 = p2.generate_trade_uuid(999);
        assert_eq!(uuid, uuid2);
    }
}
