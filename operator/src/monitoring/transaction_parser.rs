//! Transaction parser for detecting swap transactions
//!
//! Parses transactions from various DEXes (Jupiter, Raydium, Orca, Pump.fun)
//! and extracts swap information.

use anyhow::{Context, Result};
use rust_decimal::prelude::*;
use serde_json::Value;

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
    pub amount_in: Decimal,
    pub amount_out: Decimal,
    pub direction: SwapDirection,
    pub dex: String,
    pub slippage: Option<f64>, // Percentage, not a financial amount
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
        .and_then(|arr| arr.first())
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

/// Check whether any instruction in the transaction mentions one of the given program IDs.
fn has_program_id(tx_json: &Value, program_ids: &[&str]) -> bool {
    let instructions = tx_json
        .get("transaction")
        .and_then(|t| t.get("message"))
        .and_then(|m| m.get("instructions"))
        .and_then(|i| i.as_array());

    // Also check inner instructions (present when programs CPI into each other)
    let inner_instructions = tx_json
        .get("meta")
        .and_then(|m| m.get("innerInstructions"))
        .and_then(|i| i.as_array());

    let check_list = |insts: &Vec<Value>| {
        insts.iter().any(|inst| {
            inst.get("programId")
                .and_then(|p| p.as_str())
                .map(|pid| program_ids.contains(&pid))
                .unwrap_or(false)
        })
    };

    instructions.map(check_list).unwrap_or(false)
        || inner_instructions
            .map(|outer| {
                outer.iter().any(|inner_group| {
                    inner_group
                        .get("instructions")
                        .and_then(|i| i.as_array())
                        .map(check_list)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
}

/// Parse Raydium swap (AMM v4 and CLMM)
fn parse_raydium_swap(tx_json: &Value, wallet_address: &str) -> Result<ParsedSwap> {
    const RAYDIUM_AMM_V4: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
    const RAYDIUM_CLMM: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

    if !has_program_id(tx_json, &[RAYDIUM_AMM_V4, RAYDIUM_CLMM]) {
        return Err(anyhow::anyhow!("Not a Raydium swap"));
    }

    let pre_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("preTokenBalances"))
        .and_then(|b| b.as_array());
    let post_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("postTokenBalances"))
        .and_then(|b| b.as_array());

    let (token_in, token_out, amount_in, amount_out, direction) =
        parse_balance_changes(pre_balances, post_balances, wallet_address)?;

    Ok(ParsedSwap {
        token_in,
        token_out,
        amount_in,
        amount_out,
        direction,
        dex: "Raydium".to_string(),
        slippage: None,
    })
}

/// Parse Orca swap (Whirlpool)
fn parse_orca_swap(tx_json: &Value, wallet_address: &str) -> Result<ParsedSwap> {
    const ORCA_WHIRLPOOL: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

    if !has_program_id(tx_json, &[ORCA_WHIRLPOOL]) {
        return Err(anyhow::anyhow!("Not an Orca swap"));
    }

    let pre_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("preTokenBalances"))
        .and_then(|b| b.as_array());
    let post_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("postTokenBalances"))
        .and_then(|b| b.as_array());

    let (token_in, token_out, amount_in, amount_out, direction) =
        parse_balance_changes(pre_balances, post_balances, wallet_address)?;

    Ok(ParsedSwap {
        token_in,
        token_out,
        amount_in,
        amount_out,
        direction,
        dex: "Orca".to_string(),
        slippage: None,
    })
}

/// Parse Pump.fun swap
fn parse_pumpfun_swap(tx_json: &Value, wallet_address: &str) -> Result<ParsedSwap> {
    const PUMPFUN: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

    if !has_program_id(tx_json, &[PUMPFUN]) {
        return Err(anyhow::anyhow!("Not a Pump.fun swap"));
    }

    let pre_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("preTokenBalances"))
        .and_then(|b| b.as_array());
    let post_balances = tx_json
        .get("meta")
        .and_then(|m| m.get("postTokenBalances"))
        .and_then(|b| b.as_array());

    let (token_in, token_out, amount_in, amount_out, direction) =
        parse_balance_changes(pre_balances, post_balances, wallet_address)?;

    Ok(ParsedSwap {
        token_in,
        token_out,
        amount_in,
        amount_out,
        direction,
        dex: "Pump.fun".to_string(),
        slippage: None,
    })
}

/// Parse balance changes to determine swap direction and amounts
fn parse_balance_changes(
    pre_balances: Option<&Vec<Value>>,
    post_balances: Option<&Vec<Value>>,
    _wallet_address: &str,
) -> Result<(String, String, Decimal, Decimal, SwapDirection)> {
    // Parse token balance changes from pre/post balances
    // Structure: Each balance entry has:
    // - accountIndex: index into accounts array
    // - mint: token mint address
    // - uiTokenAmount: { uiAmount, decimals, amount }

    // Use empty vectors as defaults to avoid lifetime issues
    let empty_pre: Vec<Value> = Vec::new();
    let empty_post: Vec<Value> = Vec::new();
    let pre_balances = pre_balances.unwrap_or(&empty_pre);
    let post_balances = post_balances.unwrap_or(&empty_post);

    // Create maps of account index -> balance for easier matching
    let mut pre_map: std::collections::HashMap<usize, (String, Decimal)> =
        std::collections::HashMap::new();
    let mut post_map: std::collections::HashMap<usize, (String, Decimal)> =
        std::collections::HashMap::new();

    // Parse pre balances
    for balance in pre_balances {
        if let (Some(account_idx), Some(mint), Some(ui_amount)) = (
            balance
                .get("accountIndex")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            balance.get("mint").and_then(|v| v.as_str()),
            balance
                .get("uiTokenAmount")
                .and_then(|v| v.get("uiAmount"))
                .and_then(|v| v.as_f64()),
        ) {
            let amount = Decimal::from_f64_retain(ui_amount).unwrap_or(Decimal::ZERO);
            pre_map.insert(account_idx, (mint.to_string(), amount));
        }
    }

    // Parse post balances
    for balance in post_balances {
        if let (Some(account_idx), Some(mint), Some(ui_amount)) = (
            balance
                .get("accountIndex")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            balance.get("mint").and_then(|v| v.as_str()),
            balance
                .get("uiTokenAmount")
                .and_then(|v| v.get("uiAmount"))
                .and_then(|v| v.as_f64()),
        ) {
            let amount = Decimal::from_f64_retain(ui_amount).unwrap_or(Decimal::ZERO);
            post_map.insert(account_idx, (mint.to_string(), amount));
        }
    }

    // Calculate balance changes
    let mut token_changes: Vec<(String, Decimal)> = Vec::new();
    let sol_mint = "So11111111111111111111111111111111111111112";

    // Check all accounts that appear in either pre or post
    let all_accounts: std::collections::HashSet<usize> =
        pre_map.keys().chain(post_map.keys()).cloned().collect();

    for account_idx in all_accounts {
        let pre_balance = pre_map
            .get(&account_idx)
            .map(|(_, amt)| *amt)
            .unwrap_or(Decimal::ZERO);
        let post_balance = post_map
            .get(&account_idx)
            .map(|(_, amt)| *amt)
            .unwrap_or(Decimal::ZERO);
        let change = post_balance - pre_balance;

        if change.abs() > Decimal::from_str("0.0001").unwrap_or(Decimal::ZERO) {
            // Significant change
            let mint = post_map
                .get(&account_idx)
                .or_else(|| pre_map.get(&account_idx))
                .map(|(m, _)| m.clone())
                .unwrap_or_default();
            token_changes.push((mint, change));
        }
    }

    // Determine swap direction and amounts
    // Find SOL change and token change
    let mut sol_change = Decimal::ZERO;
    let mut token_change: Option<(String, Decimal)> = None;

    for (mint, change) in &token_changes {
        if mint == sol_mint {
            sol_change = *change;
        } else {
            token_change = Some((mint.clone(), *change));
        }
    }

    // Determine direction: SOL going out = BUY (buying token), SOL coming in = SELL (selling token)
    let direction = if sol_change < Decimal::ZERO {
        SwapDirection::Buy // SOL decreased, buying token
    } else {
        SwapDirection::Sell // SOL increased, selling token
    };

    // Extract amounts
    let (token_in, token_out, amount_in, amount_out) =
        if let Some((token_mint, token_amt)) = token_change {
            if direction == SwapDirection::Buy {
                // Buying: SOL -> Token
                (
                    sol_mint.to_string(),
                    token_mint,
                    sol_change.abs(),
                    token_amt.abs(),
                )
            } else {
                // Selling: Token -> SOL
                (
                    token_mint,
                    sol_mint.to_string(),
                    token_amt.abs(),
                    sol_change.abs(),
                )
            }
        } else {
            // Fallback if we can't determine token
            (
                sol_mint.to_string(),
                "".to_string(),
                sol_change.abs(),
                Decimal::ZERO,
            )
        };

    Ok((token_in, token_out, amount_in, amount_out, direction))
}

/// Parse Helius webhook payload to extract swap information
pub fn parse_helius_webhook(
    payload: &crate::monitoring::helius::HeliusWebhookPayload,
) -> Result<Option<ParsedSwap>> {
    // Check if this is a SWAP transaction
    if payload.transaction_type != "SWAP" {
        return Ok(None);
    }

    // Parse token balance changes
    for account in &payload.account_data {
        if let Some(token_changes) = &account.token_balance_changes {
            if let Some(change) = token_changes.first() {
                // Determine direction based on balance change
                // Positive change = received tokens (BUY)
                // Negative change = sent tokens (SELL)
                let amount_str = &change.raw_token_amount.token_amount;
                let amount = Decimal::from_str(amount_str).unwrap_or(Decimal::ZERO);

                let direction = if amount > Decimal::ZERO {
                    SwapDirection::Buy
                } else {
                    SwapDirection::Sell
                };

                // Check native balance change for SOL amount
                let sol_amount = account
                    .native_balance_change
                    .map(|c| Decimal::from(c) / Decimal::from(1_000_000_000u64))
                    .unwrap_or(Decimal::ZERO);

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
                    dex: "Unknown".to_string(),
                    slippage: None,
                }));
            }
        }
    }

    Ok(None)
}
