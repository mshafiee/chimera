//! Transaction parser for detecting swap transactions
//!
//! Parses transactions from various DEXes (Jupiter, Raydium, Orca, Pump.fun)
//! and extracts swap information.

use serde_json::Value;
use anyhow::{Context, Result};

/// Transaction information
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub signature: String,
    pub wallet_address: String,
    pub parsed_swap: Option<ParsedSwap>,
}

/// Parsed swap information
#[derive(Debug, Clone)]
pub struct ParsedSwap {
    pub token_in: String,
    pub token_out: String,
    pub amount_in: f64,
    pub amount_out: f64,
    pub direction: SwapDirection,
    pub dex: String,
    pub slippage: Option<f64>,
}

/// Swap direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapDirection {
    /// Buying token (SOL -> Token)
    Buy,
    /// Selling token (Token -> SOL)
    Sell,
}

/// Parse transaction to detect swaps
pub fn parse_transaction(tx_json: &Value, wallet_address: &str) -> Result<TransactionInfo> {
    let signature = tx_json
        .get("transaction")
        .and_then(|t| t.get("signatures"))
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.get(0))
        .and_then(|s| s.as_str())
        .context("Missing transaction signature")?
        .to_string();

    // Try to parse as Jupiter swap
    if let Ok(swap) = parse_jupiter_swap(tx_json, wallet_address) {
        return Ok(TransactionInfo {
            signature,
            wallet_address: wallet_address.to_string(),
            parsed_swap: Some(swap),
        });
    }

    // Try to parse as Raydium swap
    if let Ok(swap) = parse_raydium_swap(tx_json, wallet_address) {
        return Ok(TransactionInfo {
            signature,
            wallet_address: wallet_address.to_string(),
            parsed_swap: Some(swap),
        });
    }

    // Try to parse as Orca swap
    if let Ok(swap) = parse_orca_swap(tx_json, wallet_address) {
        return Ok(TransactionInfo {
            signature,
            wallet_address: wallet_address.to_string(),
            parsed_swap: Some(swap),
        });
    }

    // Try to parse as Pump.fun swap
    if let Ok(swap) = parse_pumpfun_swap(tx_json, wallet_address) {
        return Ok(TransactionInfo {
            signature,
            wallet_address: wallet_address.to_string(),
            parsed_swap: Some(swap),
        });
    }

    // No swap detected
    Ok(TransactionInfo {
        signature,
        wallet_address: wallet_address.to_string(),
        parsed_swap: None,
    })
}

/// Parse Jupiter swap transaction
fn parse_jupiter_swap(tx_json: &Value, wallet_address: &str) -> Result<ParsedSwap> {
    // Jupiter swaps typically have specific program IDs
    // Check for Jupiter program ID in instructions
    let instructions = tx_json
        .get("transaction")
        .and_then(|t| t.get("message"))
        .and_then(|m| m.get("instructions"))
        .and_then(|i| i.as_array())
        .context("Missing instructions")?;

    // Look for Jupiter program ID: JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4
    let jupiter_program = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
    
    let mut found_jupiter = false;
    for inst in instructions {
        if let Some(program_id) = inst.get("programId") {
            if program_id.as_str() == Some(jupiter_program) {
                found_jupiter = true;
                break;
            }
        }
    }

    if !found_jupiter {
        return Err(anyhow::anyhow!("Not a Jupiter swap"));
    }

    // Parse token accounts and amounts from pre/post token balances
    // This is simplified - full parsing would need account data
    let pre_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("preTokenBalances"))
        .and_then(|b| b.as_array());

    let post_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("postTokenBalances"))
        .and_then(|b| b.as_array());

    // Determine direction and amounts from balance changes
    // This is a simplified parser - production would need more sophisticated parsing
    let (token_in, token_out, amount_in, amount_out, direction) = 
        parse_balance_changes(pre_balances, post_balances, wallet_address)?;

    Ok(ParsedSwap {
        token_in,
        token_out,
        amount_in,
        amount_out,
        direction,
        dex: "Jupiter".to_string(),
        slippage: None,
    })
}

/// Parse Raydium swap
fn parse_raydium_swap(_tx_json: &Value, _wallet_address: &str) -> Result<ParsedSwap> {
    // Similar to Jupiter but check for Raydium program IDs
    // Raydium AMM: 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8
    Err(anyhow::anyhow!("Raydium parsing not fully implemented"))
}

/// Parse Orca swap
fn parse_orca_swap(_tx_json: &Value, _wallet_address: &str) -> Result<ParsedSwap> {
    // Similar to Jupiter but check for Orca program IDs
    Err(anyhow::anyhow!("Orca parsing not fully implemented"))
}

/// Parse Pump.fun swap
fn parse_pumpfun_swap(_tx_json: &Value, _wallet_address: &str) -> Result<ParsedSwap> {
    // Pump.fun has specific program ID: 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P
    Err(anyhow::anyhow!("Pump.fun parsing not fully implemented"))
}

/// Parse balance changes to determine swap direction and amounts
fn parse_balance_changes(
    pre_balances: Option<&Vec<Value>>,
    post_balances: Option<&Vec<Value>>,
    wallet_address: &str,
) -> Result<(String, String, f64, f64, SwapDirection)> {
    // This is a simplified parser
    // In production, would need to:
    // 1. Match pre/post balances by account
    // 2. Calculate differences
    // 3. Determine which token is SOL (native)
    // 4. Determine direction based on SOL flow

    // For now, return placeholder
    // TODO: Implement full balance change parsing
    Ok((
        "So11111111111111111111111111111111111111112".to_string(), // SOL
        "".to_string(),
        0.0,
        0.0,
        SwapDirection::Buy,
    ))
}

/// Parse Helius webhook payload to extract swap information
pub fn parse_helius_webhook(payload: &crate::monitoring::helius::HeliusWebhookPayload) -> Result<Option<ParsedSwap>> {
    // Check if this is a SWAP transaction
    if payload.transaction_type != "SWAP" {
        return Ok(None);
    }

    // Parse token balance changes
    for account in &payload.account_data {
        if let Some(token_changes) = &account.token_balance_changes {
            for change in token_changes {
                // Determine direction based on balance change
                // Positive change = received tokens (BUY)
                // Negative change = sent tokens (SELL)
                let amount_str = &change.raw_token_amount.token_amount;
                let amount: f64 = amount_str.parse().unwrap_or(0.0);

                let direction = if amount > 0.0 {
                    SwapDirection::Buy
                } else {
                    SwapDirection::Sell
                };

                // Check native balance change for SOL amount
                let sol_amount = account.native_balance_change
                    .map(|c| c as f64 / 1e9) // Convert lamports to SOL
                    .unwrap_or(0.0);

                return Ok(Some(ParsedSwap {
                    token_in: if direction == SwapDirection::Buy {
                        "So11111111111111111111111111111111111111112".to_string()
                    } else {
                        change.mint.clone()
                    },
                    token_out: if direction == SwapDirection::Buy {
                        change.mint.clone()
                    } else {
                        "So11111111111111111111111111111111111111112".to_string()
                    },
                    amount_in: sol_amount.abs(),
                    amount_out: amount.abs(),
                    direction,
                    dex: "Unknown".to_string(), // Helius doesn't specify DEX
                    slippage: None,
                }));
            }
        }
    }

    Ok(None)
}
