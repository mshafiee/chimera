//! Export utilities for trades (CSV and PDF)
//! Moved from db.rs as part of the database layer refactoring.

use super::types::TradeDetail;
use chimera_core::error::{AppError, AppResult};

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Generate CSV content from trades
pub fn trades_to_csv(trades: &[TradeDetail]) -> String {
    let mut csv = String::new();

    // Header
    csv.push_str("id,trade_uuid,wallet_address,token_address,token_symbol,strategy,side,amount_sol,price_at_signal,tx_signature,status,pnl_sol,pnl_usd,jito_tip_sol,dex_fee_sol,slippage_cost_sol,total_cost_sol,net_pnl_sol,created_at\n");

    // Data rows
    for trade in trades {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            trade.id,
            trade.trade_uuid,
            trade.wallet_address,
            trade.token_address,
            csv_escape(trade.token_symbol.as_deref().unwrap_or("")),
            trade.strategy,
            trade.side,
            trade.amount_sol,
            trade
                .price_at_signal
                .as_ref()
                .map_or(String::default(), |p| p.to_string()),
            trade.tx_signature.as_deref().unwrap_or(""),
            trade.status,
            trade.pnl_sol.map(|p| p.to_string()).unwrap_or_default(),
            trade.pnl_usd.map(|p| p.to_string()).unwrap_or_default(),
            trade
                .jito_tip_sol
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade.dex_fee_sol.map(|p| p.to_string()).unwrap_or_default(),
            trade
                .slippage_cost_sol
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade
                .total_cost_sol
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade.net_pnl_sol.map(|p| p.to_string()).unwrap_or_default(),
            trade.created_at,
        ));
    }

    csv
}

/// Generate PDF content from trades
pub fn trades_to_pdf(_trades: &[TradeDetail]) -> AppResult<Vec<u8>> {
    Err(AppError::Internal(
        "PDF export not available in this printpdf version".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn trade_detail(uuid: &str, token_symbol: Option<&str>) -> TradeDetail {
        TradeDetail {
            id: 1,
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet".to_string(),
            token_address: "token".to_string(),
            token_symbol: token_symbol.map(|s| s.to_string()),
            strategy: "SHIELD".to_string(),
            side: "BUY".to_string(),
            amount_sol: Decimal::from(10),
            price_at_signal: None,
            tx_signature: None,
            status: "PENDING".to_string(),
            retry_count: 0,
            error_message: None,
            pnl_sol: None,
            pnl_usd: None,
            jito_tip_sol: None,
            dex_fee_sol: None,
            slippage_cost_sol: None,
            total_cost_sol: None,
            net_pnl_sol: None,
            pnl_data_valid: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_csv_escape() {
        assert_eq!(csv_escape("simple"), "simple");
        assert_eq!(csv_escape("with,comma"), "\"with,comma\"");
        assert_eq!(csv_escape("has \"quote\""), "\"has \"\"quote\"\"\"");
        assert_eq!(csv_escape("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn test_trades_to_csv_header_and_row() {
        let csv = trades_to_csv(&[trade_detail("t1", Some("TOKEN"))]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("id,trade_uuid,wallet_address"));
        assert!(lines[1].contains("t1"));
        assert!(lines[1].contains("TOKEN"));
        assert!(lines[1].contains("SHIELD"));
    }

    #[test]
    fn test_trades_to_csv_empty() {
        let csv = trades_to_csv(&[]);
        // Only the header line.
        assert_eq!(csv.lines().count(), 1);
    }

    #[test]
    fn test_trades_to_csv_escapes_symbol_with_comma() {
        let csv = trades_to_csv(&[trade_detail("t1", Some("A,B"))]);
        assert!(csv.contains("\"A,B\""));
    }

    #[test]
    fn test_trades_to_pdf_returns_error() {
        let result = trades_to_pdf(&[trade_detail("t1", None)]);
        assert!(result.is_err());
    }
}
