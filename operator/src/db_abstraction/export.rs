//! Export utilities for trades (CSV and PDF)
//! Moved from db.rs as part of the database layer refactoring.

use super::types::TradeDetail;
use crate::error::{AppError, AppResult};

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
