use rust_decimal::prelude::*;

use crate::error::AppError;

/// Whether `CHIMERA_DEV_MODE` is set to a truthy value.
///
/// Only `1`, `true`, `yes`, `on` (case-insensitive) enable dev mode; `0`,
/// `false`, empty, or unset all mean OFF (production-safe). Call sites
/// previously used `var("CHIMERA_DEV_MODE").is_ok()`, which treated the
/// documented "disable" value `CHIMERA_DEV_MODE=0` as dev-mode-ON — silently
/// skipping `config.validate()` and the honeypot fail-closed in production.
pub fn is_dev_mode() -> bool {
    match std::env::var("CHIMERA_DEV_MODE") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Helius API base URL from env var with fallback
pub fn helius_api_base_url() -> String {
    std::env::var("HELIUS_API_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://api.helius.xyz/v0".into())
}

/// Helius **Solana RPC** endpoint (with the API key), used for JSON-RPC bundle
/// methods (`sendBundle`, `getBundleStatuses`). Per Helius docs these live at
/// the RPC host (`mainnet.helius-rpc.com`), NOT at `api.helius.xyz/v0`.
/// Overridable via `HELIUS_RPC_BASE_URL`.
///
/// The key is percent-encoded so a key containing `&`, `#`, or `=` cannot
/// produce a malformed/ambiguous URL.
pub fn helius_rpc_url(api_key: &str) -> String {
    let base = std::env::var("HELIUS_RPC_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://mainnet.helius-rpc.com".into());
    format!("{}?api-key={}", base, urlencoding::encode(api_key))
}

/// Safely convert SOL (Decimal) to Lamports (u64) using Decimal to avoid precision loss
pub fn sol_to_lamports(sol: Decimal) -> Result<u64, AppError> {
    if sol.is_sign_negative() {
        return Err(AppError::InvalidInput(format!(
            "Negative SOL value: {} SOL cannot be negative",
            sol
        )));
    }
    // 1 SOL = 1,000,000,000 Lamports
    let multiplier = Decimal::new(1_000_000_000, 0);
    let result = sol * multiplier;

    result.to_u64().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "Decimal conversion overflow: {} SOL exceeds u64 max",
            sol
        ))
    })
}

/// Safely convert SOL (f64) to Lamports (u64) using Decimal to avoid precision loss
/// This is a convenience function for legacy code that still uses f64
pub fn sol_to_lamports_f64(sol: f64) -> Result<u64, AppError> {
    // Convert float to Decimal first to handle precision safely
    let sol_decimal = Decimal::from_f64_retain(sol)
        .ok_or_else(|| AppError::InvalidInput(format!("Cannot represent {} as a Decimal", sol)))?;
    sol_to_lamports(sol_decimal)
}

/// Safely convert Lamports (u64) to SOL (f64) for display/DB
pub fn lamports_to_sol(lamports: u64) -> f64 {
    let lamports_dec = Decimal::from(lamports);
    let divisor = Decimal::new(1_000_000_000, 0);

    (lamports_dec / divisor).to_f64().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    /// Serializes the env-var-mutating tests so parallel test threads cannot
    /// race each other (or other readers) on the process env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_env(key: &str, value: Option<&str>) {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn test_is_dev_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        for (val, expected) in [
            (None, false),
            (Some(""), false),
            (Some("0"), false),
            (Some("false"), false),
            (Some("off"), false),
            (Some("garbage"), false),
            (Some("1"), true),
            (Some("true"), true),
            (Some("YES"), true),
            (Some("  on  "), true),
        ] {
            set_env("CHIMERA_DEV_MODE", val);
            assert_eq!(is_dev_mode(), expected, "CHIMERA_DEV_MODE={:?}", val);
        }
    }

    #[test]
    fn test_helius_api_base_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        set_env("HELIUS_API_BASE_URL", None);
        assert_eq!(helius_api_base_url(), "https://api.helius.xyz/v0");
        set_env("HELIUS_API_BASE_URL", Some(""));
        assert_eq!(helius_api_base_url(), "https://api.helius.xyz/v0");
        set_env("HELIUS_API_BASE_URL", Some("https://custom.helius.xyz/v2"));
        assert_eq!(helius_api_base_url(), "https://custom.helius.xyz/v2");
    }

    #[test]
    fn test_helius_rpc_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        set_env("HELIUS_RPC_BASE_URL", None);
        assert_eq!(
            helius_rpc_url("abc123"),
            "https://mainnet.helius-rpc.com?api-key=abc123"
        );
        set_env("HELIUS_RPC_BASE_URL", Some(""));
        assert_eq!(
            helius_rpc_url("abc123"),
            "https://mainnet.helius-rpc.com?api-key=abc123"
        );
        set_env("HELIUS_RPC_BASE_URL", Some("https://rpc.example.com"));
        // API key is percent-encoded
        assert_eq!(
            helius_rpc_url("a&b=c#d"),
            "https://rpc.example.com?api-key=a%26b%3Dc%23d"
        );
    }

    #[test]
    fn test_sol_to_lamports() {
        assert_eq!(sol_to_lamports(dec!(0)).unwrap(), 0);
        assert_eq!(sol_to_lamports(dec!(1)).unwrap(), 1_000_000_000);
        assert_eq!(sol_to_lamports(dec!(1.5)).unwrap(), 1_500_000_000);
        assert_eq!(sol_to_lamports(dec!(0.000000001)).unwrap(), 1);
    }

    #[test]
    fn test_sol_to_lamports_negative() {
        let err = sol_to_lamports(dec!(-1)).unwrap_err();
        assert!(err.to_string().contains("Negative SOL value"));
    }

    #[test]
    fn test_sol_to_lamports_overflow() {
        // 2e10 SOL * 1e9 lamports = 2e19 > u64::MAX (~1.84e19)
        let err = sol_to_lamports(dec!(20000000000)).unwrap_err();
        assert!(err.to_string().contains("overflow"));
    }

    #[test]
    fn test_sol_to_lamports_f64() {
        assert_eq!(sol_to_lamports_f64(0.0).unwrap(), 0);
        assert_eq!(sol_to_lamports_f64(2.0).unwrap(), 2_000_000_000);
        // Negative propagates from sol_to_lamports
        assert!(sol_to_lamports_f64(-0.5).is_err());
    }

    #[test]
    fn test_sol_to_lamports_f64_unrepresentable() {
        // NaN cannot be represented as a Decimal
        let err = sol_to_lamports_f64(f64::NAN).unwrap_err();
        assert!(err.to_string().contains("Cannot represent"));
    }

    #[test]
    fn test_sol_to_lamports_f64_overflow() {
        let err = sol_to_lamports_f64(3e10).unwrap_err();
        assert!(err.to_string().contains("overflow"));
    }

    #[test]
    fn test_lamports_to_sol() {
        assert_eq!(lamports_to_sol(0), 0.0);
        assert_eq!(lamports_to_sol(1_000_000_000), 1.0);
        assert_eq!(lamports_to_sol(1_500_000_000), 1.5);
        // Huge values still convert (f64 precision loss is expected here)
        assert_eq!(lamports_to_sol(u64::MAX), 1.8446744073709552e10);
    }
}
