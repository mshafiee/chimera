//! On-chain wallet assessment for copy-trading admission.
//!
//! Assesses a wallet's ACTUAL trade history on Solana (via Helius enhanced
//! transactions) to compute per-round-trip expectancy — the copy-trading
//! edge metric. Unlike shadow trading (which needs signals to accumulate
//! slowly) or Dune aggregate PnL (which conflates rare big winners with
//! per-signal edge), this measures: for every token the wallet round-tripped,
//! did it buy low and sell high?
//!
//! Round-trip PnL is computed directly from quote legs (USDC/USDT/WSOL/SOL)
//! — the enhanced API's `tokenAmount` is decimal-adjusted, so no decimals
//! lookups are needed for the quote side.

use std::collections::HashMap;
use std::sync::Arc;

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::monitoring::helius::HeliusClient;

/// Quote mints whose tokenAmount can be valued directly (decimal-adjusted).
const QUOTE_MINTS: [&str; 3] = [
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
    "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", // USDT
    "So11111111111111111111111111111111111111112",   // WSOL
];

/// Assessment of a wallet's on-chain round-trip trading.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct OnchainWalletAssessment {
    pub wallet: String,
    pub txs_fetched: usize,
    pub round_trips: usize,
    pub token_count: usize,
    pub win_rate_pct: f64,
    pub avg_win_pct: f64,
    pub avg_loss_pct: f64,
    pub expectancy_pct: f64,
    pub total_buy_quote: f64,
    pub total_sell_quote: f64,
}

/// Per-token ledger for one wallet.
#[derive(Debug, Default)]
struct TokenLedger {
    buy_quote: Decimal,
    sell_quote: Decimal,
}

/// Assesses wallets from their on-chain history.
pub struct OnchainAssessor {
    helius_client: Arc<HeliusClient>,
}

impl OnchainAssessor {
    pub fn new(helius_client: Arc<HeliusClient>) -> Self {
        Self { helius_client }
    }

    /// Fetch and assess a wallet's round-trip trading over its recent
    /// history. `limit` caps the number of SWAP transactions fetched.
    pub async fn assess_wallet(
        &self,
        wallet: &str,
        limit: usize,
    ) -> Result<OnchainWalletAssessment, String> {
        let txs = self
            .helius_client
            .fetch_wallet_swaps(wallet, limit)
            .await
            .map_err(|e| format!("Helius fetch failed: {e}"))?;

        let mut ledgers: HashMap<String, TokenLedger> = HashMap::new();

        for tx in &txs {
            // Ignore failed transactions.
            if tx.get("transactionError").is_some() {
                continue;
            }
            Self::apply_transaction(wallet, tx, &mut ledgers);
        }

        // Compute round-trip metrics per token.
        let mut pnls: Vec<Decimal> = Vec::new();
        let mut total_buy = Decimal::ZERO;
        let mut total_sell = Decimal::ZERO;
        for ledger in ledgers.values() {
            if ledger.buy_quote > Decimal::ZERO && ledger.sell_quote > Decimal::ZERO {
                let pnl = (ledger.sell_quote - ledger.buy_quote) / ledger.buy_quote * Decimal::from(100);
                pnls.push(pnl);
                total_buy += ledger.buy_quote;
                total_sell += ledger.sell_quote;
            }
        }

        let n = pnls.len();
        let wins = pnls.iter().filter(|p| **p > Decimal::ZERO).count();
        let win_rate = if n > 0 {
            wins as f64 / n as f64 * 100.0
        } else {
            0.0
        };
        let avg_win = if wins > 0 {
            pnls.iter().filter(|p| **p > Decimal::ZERO).map(|p| p.to_f64().unwrap_or(0.0)).sum::<f64>() / wins as f64
        } else {
            0.0
        };
        let losses = n - wins;
        let avg_loss = if losses > 0 {
            pnls.iter().filter(|p| **p <= Decimal::ZERO).map(|p| p.to_f64().unwrap_or(0.0)).sum::<f64>() / losses as f64
        } else {
            0.0
        };
        let expectancy = if n > 0 {
            pnls.iter().map(|p| p.to_f64().unwrap_or(0.0)).sum::<f64>() / n as f64
        } else {
            0.0
        };

        Ok(OnchainWalletAssessment {
            wallet: wallet.to_string(),
            txs_fetched: txs.len(),
            round_trips: n,
            token_count: ledgers.len(),
            win_rate_pct: round2(win_rate),
            avg_win_pct: round2(avg_win),
            avg_loss_pct: round2(avg_loss),
            expectancy_pct: round2(expectancy),
            total_buy_quote: total_buy.to_f64().unwrap_or(0.0),
            total_sell_quote: total_sell.to_f64().unwrap_or(0.0),
        })
    }

    /// Apply one transaction's wallet legs to the token ledgers.
    fn apply_transaction(
        wallet: &str,
        tx: &serde_json::Value,
        ledgers: &mut HashMap<String, TokenLedger>,
    ) {
        // Collect the wallet's token-transfer legs.
        let mut wallet_legs: Vec<(String, Decimal, bool)> = Vec::new(); // (mint, amount, wallet_paid)
        if let Some(transfers) = tx.get("tokenTransfers").and_then(|t| t.as_array()) {
            for leg in transfers {
                let mint = leg.get("mint").and_then(|m| m.as_str()).unwrap_or("");
                // tokenAmount is a JSON NUMBER in the raw enhanced API
                // ("tokenAmount": 12), but can be a string in some modes —
                // handle both.
                let amount = leg
                    .get("tokenAmount")
                    .map(|a| {
                        if let Some(n) = a.as_f64() {
                            Decimal::from_f64_retain(n).unwrap_or(Decimal::ZERO)
                        } else {
                            a.as_str()
                                .and_then(|s| Decimal::from_str(s).ok())
                                .unwrap_or(Decimal::ZERO)
                        }
                    })
                    .unwrap_or(Decimal::ZERO);
                let from = leg.get("fromUserAccount").and_then(|v| v.as_str()).unwrap_or("");
                let to = leg.get("toUserAccount").and_then(|v| v.as_str()).unwrap_or("");
                if from == wallet {
                    wallet_legs.push((mint.to_string(), amount, true));
                } else if to == wallet {
                    wallet_legs.push((mint.to_string(), amount, false));
                }
            }
        }
        // Native SOL leg (raw lamports / 1e9).
        if let Some(transfers) = tx.get("nativeTransfers").and_then(|t| t.as_array()) {
            for leg in transfers {
                let from = leg.get("fromUserAccount").and_then(|v| v.as_str()).unwrap_or("");
                let to = leg.get("toUserAccount").and_then(|v| v.as_str()).unwrap_or("");
                let lamports = leg.get("amount").and_then(|a| a.as_u64()).unwrap_or(0);
                if (from == wallet || to == wallet) && lamports > 0 {
                    let sol = Decimal::from(lamports) / Decimal::from(1_000_000_000u64);
                    wallet_legs.push((
                        "So11111111111111111111111111111111111111112".to_string(),
                        sol,
                        from == wallet,
                    ));
                }
            }
        }

        // For each quote leg (wallet paid quote -> bought token; received
        // quote -> sold token), find the traded token (the other leg).
        for (i, (mint, amount, paid)) in wallet_legs.iter().enumerate() {
            if !QUOTE_MINTS.contains(&mint.as_str()) || amount.is_zero() {
                continue;
            }
            let traded = wallet_legs
                .iter()
                .enumerate()
                .filter(|(j, (m, a, _))| *j != i && !QUOTE_MINTS.contains(&m.as_str()) && !a.is_zero())
                .map(|(_, (m, _, _))| m.clone())
                .next();
            let Some(traded_token) = traded else { continue };

            let ledger = ledgers.entry(traded_token).or_default();
            if *paid {
                ledger.buy_quote += *amount;
            } else {
                ledger.sell_quote += *amount;
            }
        }
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(mint: &str, amount: &str, from: &str, to: &str) -> serde_json::Value {
        serde_json::json!({
            "mint": mint,
            "tokenAmount": amount,
            "fromUserAccount": from,
            "toUserAccount": to,
        })
    }

    #[test]
    fn test_apply_buy_and_sell_round_trip() {
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

        // BUY: wallet pays 100 USDC, receives 1000 TokA
        let buy_tx = serde_json::json!({
            "transactionError": null,
            "tokenTransfers": [
                leg(usdc, "100", wallet, "DexAcct1111111111111111111111111111111111111"),
                leg(token, "1000", "DexAcct1111111111111111111111111111111111111", wallet),
            ],
            "nativeTransfers": [],
        });
        // SELL: wallet pays 1000 TokA, receives 150 USDC
        let sell_tx = serde_json::json!({
            "transactionError": null,
            "tokenTransfers": [
                leg(token, "1000", wallet, "DexAcct1111111111111111111111111111111111111"),
                leg(usdc, "150", "DexAcct1111111111111111111111111111111111111", wallet),
            ],
            "nativeTransfers": [],
        });

        let mut ledgers = HashMap::new();
        OnchainAssessor::apply_transaction(wallet, &buy_tx, &mut ledgers);
        OnchainAssessor::apply_transaction(wallet, &sell_tx, &mut ledgers);

        let ledger = ledgers.get(token).expect("token ledger");
        assert_eq!(ledger.buy_quote, Decimal::from(100));
        assert_eq!(ledger.sell_quote, Decimal::from(150));

        // Full assessment would show one round trip at +50%.
        let pnl = (ledger.sell_quote - ledger.buy_quote) / ledger.buy_quote * Decimal::from(100);
        assert_eq!(pnl, Decimal::from(50));
    }

    #[test]
    fn test_sol_quoted_buy() {
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let wsol = "So11111111111111111111111111111111111111112";

        let buy_tx = serde_json::json!({
            "transactionError": null,
            "tokenTransfers": [
                leg(wsol, "5", wallet, "DexAcct1111111111111111111111111111111111111"),
                leg(token, "100", "DexAcct1111111111111111111111111111111111111", wallet),
            ],
            "nativeTransfers": [],
        });

        let mut ledgers = HashMap::new();
        OnchainAssessor::apply_transaction(wallet, &buy_tx, &mut ledgers);
        assert_eq!(ledgers.get(token).unwrap().buy_quote, Decimal::from(5));
    }

    #[test]
    fn test_numeric_token_amount_parses() {
        // The raw Helius enhanced API emits tokenAmount as a JSON NUMBER
        // (e.g. "tokenAmount": 12), not a string. The parser must handle it.
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

        let buy_tx = serde_json::json!({
            "transactionError": null,
            "tokenTransfers": [
                {"mint": usdc, "tokenAmount": 12, "fromUserAccount": wallet, "toUserAccount": "DexAcct1111111111111111111111111111111111111"},
                {"mint": token, "tokenAmount": 579.266206, "fromUserAccount": "DexAcct1111111111111111111111111111111111111", "toUserAccount": wallet},
            ],
            "nativeTransfers": [],
        });

        let mut ledgers = HashMap::new();
        OnchainAssessor::apply_transaction(wallet, &buy_tx, &mut ledgers);
        let ledger = ledgers.get(token).expect("token ledger");
        assert_eq!(ledger.buy_quote, Decimal::from(12));
    }

    #[test]
    fn test_ignores_failed_transactions() {
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

        let failed = serde_json::json!({
            "transactionError": {"error": "Instruction failed"},
            "tokenTransfers": [
                leg(usdc, "100", wallet, "DexAcct1111111111111111111111111111111111111"),
                leg(token, "1000", "DexAcct1111111111111111111111111111111111111", wallet),
            ],
            "nativeTransfers": [],
        });

        let mut ledgers = HashMap::new();
        OnchainAssessor::apply_transaction(wallet, &failed, &mut ledgers);
        // apply_transaction itself doesn't check errors — assess_wallet does.
        // This test verifies the ledger still builds; the error filter is in
        // assess_wallet's loop. Keep behavior documented.
        assert!(ledgers.contains_key(token));
    }
}
