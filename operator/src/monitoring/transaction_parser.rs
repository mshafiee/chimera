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

/// Parse Helius LaserStream WebSocket message to extract swap details
///
/// LaserStream pushes fully enriched transaction data with tokenTransfers,
/// nativeTransfers, and balance changes. This function extracts swap information
/// directly from the WSS payload without needing additional RPC calls.
pub fn parse_laserstream_message(
    payload: &Value,
    wallet_address: &str,
) -> Result<Option<ParsedSwap>> {
    // LaserStream provides tokenTransfers array with parsed transfer data
    let token_transfers = payload
        .get("tokenTransfers")
        .and_then(|t| t.as_array())
        .context("Missing tokenTransfers in LaserStream payload")?;

    let mut token_in = String::new();
    let mut token_out = String::new();
    let mut amount_in = Decimal::ZERO;
    let mut amount_out = Decimal::ZERO;

    // Process token transfers to find swap
    for transfer in token_transfers {
        let from_user = transfer.get("fromUserAccount").and_then(|a| a.as_str());
        let to_user = transfer.get("toUserAccount").and_then(|a| a.as_str());
        let mint = transfer
            .get("mint")
            .and_then(|m| m.as_str())
            .context("Missing mint in token transfer")?;
        let token_amount_str = transfer
            .get("tokenAmount")
            .and_then(|a| a.as_str())
            .context("Missing tokenAmount in token transfer")?;

        // Parse token amount (string to avoid precision loss)
        let amount = Decimal::from_str(token_amount_str).unwrap_or(Decimal::ZERO);

        // Track tokens sent from wallet (token_in) and received by wallet (token_out)
        if from_user == Some(wallet_address) && amount > Decimal::ZERO {
            token_in = mint.to_string();
            amount_in = amount;
        } else if to_user == Some(wallet_address) && amount > Decimal::ZERO {
            token_out = mint.to_string();
            amount_out = amount;
        }
    }

    // Validate we found both sides of the swap
    if token_in.is_empty() || token_out.is_empty() {
        tracing::debug!(
            wallet = %wallet_address,
            "Incomplete swap data in LaserStream payload"
        );
        return Ok(None);
    }

    // Determine swap direction
    let sol_mint = "So11111111111111111111111111111111111111112";
    let direction = if token_in == sol_mint {
        SwapDirection::Buy // SOL -> Token
    } else {
        SwapDirection::Sell // Token -> SOL
    };

    // Detect DEX from transaction data (optional enhancement)
    let dex = detect_dex_from_laserstream(payload)?;

    Ok(Some(ParsedSwap {
        token_in,
        token_out,
        amount_in,
        amount_out,
        direction,
        dex,
        slippage: None, // Could be calculated from price data if available
    }))
}

/// Detect DEX from LaserStream transaction data
fn detect_dex_from_laserstream(payload: &Value) -> Result<String> {
    // Check for DEX program IDs in transaction logs
    if let Some(logs) = payload.get("logs").and_then(|l| l.as_array()) {
        // Convert JsonValue array to string array before joining
        let log_strings: Vec<&str> = logs
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        let log_str = log_strings.join(" ");

        if log_str.contains("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4") {
            return Ok("Jupiter".to_string());
        } else if log_str.contains("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8") {
            return Ok("Raydium".to_string());
        } else if log_str.contains("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc") {
            return Ok("Orca".to_string());
        } else if log_str.contains("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P") {
            return Ok("Pump.fun".to_string());
        }
    }

    Ok("Unknown".to_string())
}

/// Parse a swap from Helius's explicit `events.swap` data. This is the
/// primary, most-reliable path: it directly identifies the swapper and
/// the exact tokens given/received, with no inference from token balance
/// changes (which fails when `userAccount` doesn't match for newly-created
/// token accounts).
fn parse_from_swap_event(
    swap: &crate::monitoring::helius::WebhookSwapEvent,
    _signature: &str,
    _tracked_wallet: Option<&str>,
    sol_mint: &str,
    stablecoin_mints: &[&str; 2],
) -> Result<Option<ParsedSwap>> {

    // Collect all token legs from inputs (given) and outputs (received).
    // Each leg is (mint, amount, is_output).
    let mut legs: Vec<(String, Decimal, bool)> = Vec::new();

    for leg in &swap.token_inputs {
        if let Some(rta) = &leg.raw_token_amount {
            let amount = Decimal::from_str(&rta.token_amount).unwrap_or(Decimal::ZERO);
            tracing::debug!(
                mint = %leg.mint,
                raw_token_amount = %rta.token_amount,
                parsed_amount = %amount,
                direction = "input",
                "swap_event leg parsed"
            );
            if amount.abs() > Decimal::new(1, 6) {
                legs.push((leg.mint.clone(), amount, false)); // given
            }
        }
    }
    for leg in &swap.token_outputs {
        if let Some(rta) = &leg.raw_token_amount {
            let amount = Decimal::from_str(&rta.token_amount).unwrap_or(Decimal::ZERO);
            tracing::debug!(
                mint = %leg.mint,
                raw_token_amount = %rta.token_amount,
                parsed_amount = %amount,
                direction = "output",
                "swap_event leg parsed"
            );
            if amount.abs() > Decimal::new(1, 6) {
                legs.push((leg.mint.clone(), amount, true)); // received
            }
        }
    }

    // Also handle native SOL legs.
    let mut sol_in = Decimal::ZERO;
    let mut sol_out = Decimal::ZERO;
    if let Some(ni) = &swap.native_input {
        if let Ok(amt) = ni.amount.parse::<u64>() {
            sol_in = Decimal::from(amt) / Decimal::from(1_000_000_000u64);
        }
    }
    if let Some(no) = &swap.native_output {
        if let Ok(amt) = no.amount.parse::<u64>() {
            sol_out = Decimal::from(amt) / Decimal::from(1_000_000_000u64);
        }
    }

    // Find the speculative token: a non-SOL, non-stablecoin mint in either
    // inputs or outputs.
    let mut spec_token: Option<(String, Decimal, bool)> = None; // (mint, amount, is_output)
    for (mint, amount, is_output) in &legs {
        if mint == sol_mint || stablecoin_mints.contains(&mint.as_str()) {
            continue;
        }
        match &spec_token {
            Some((_, prev, _)) if prev.abs() >= amount.abs() => {}
            _ => spec_token = Some((mint.clone(), *amount, *is_output)),
        }
    }

    let (token_mint, _token_amount, is_output) = match spec_token {
        Some(v) => v,
        None => {
            // Diagnostic: log what legs we saw so we can see why no
            // speculative token was found.
            let leg_summary: Vec<String> = legs
                .iter()
                .map(|(m, a, out)| format!("{}={}({})", &m[..m.len().min(8)], a, if *out { "out" } else { "in" }))
                .collect();
            tracing::debug!(
                _sig = _signature,
                legs = leg_summary.join(", "),
                sol_in = %sol_in,
                sol_out = %sol_out,
                "parse_from_swap_event: no speculative token found — see legs"
            );
            return Ok(None);
        }
    };

    // Direction: received (output) = BUY, given (input) = SELL.
    let direction = if is_output {
        SwapDirection::Buy
    } else {
        SwapDirection::Sell
    };

    // Quote amount: SOL or stablecoin side.
    let (quote_mint, quote_amount) = {
        // Check stablecoin legs first.
        let mut best_quote: Option<(String, Decimal)> = None;
        for (mint, amount, _) in &legs {
            if stablecoin_mints.contains(&mint.as_str())
                && best_quote.as_ref().map(|(_, a)| a.abs()).unwrap_or(Decimal::ZERO) < amount.abs()
            {
                best_quote = Some((mint.clone(), *amount));
            }
        }
        if let Some(q) = best_quote {
            q
        } else {
            // Use SOL (native).
            let sol_amount = if sol_out > Decimal::ZERO {
                sol_out
            } else {
                sol_in
            };
            (sol_mint.to_string(), sol_amount)
        }
    };

    let dex = "Unknown";

    tracing::info!(
        token_mint = %token_mint,
        direction = ?direction,
        quote_mint = %quote_mint,
        quote_amount = %quote_amount,
        "parse_from_swap_event SUCCESS — returning ParsedSwap"
    );

    Ok(Some(ParsedSwap {
        token_in: if direction == SwapDirection::Buy {
            quote_mint.clone()
        } else {
            token_mint.clone()
        },
        token_out: if direction == SwapDirection::Buy {
            token_mint.clone()
        } else {
            quote_mint.clone()
        },
        amount_in: quote_amount,
        amount_out: quote_amount, // approximate; exact ratio not critical for signal detection
        direction,
        dex: dex.to_string(),
        slippage: None,
    }))
}

/// Parse Helius webhook payload to extract swap information
pub fn parse_helius_webhook(
    payload: &crate::monitoring::helius::HeliusWebhookPayload,
    tracked_wallet: Option<&str>,
) -> Result<Option<ParsedSwap>> {
    // Check if this is a SWAP transaction
    if payload.transaction_type != "SWAP" {
        return Ok(None);
    }

    const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
    const STABLECOIN_MINTS: [&str; 2] = [
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", // USDT
    ];

    // Primary: use the explicit `events.swap` data when present. Helius
    // enriched webhooks include a structured swap event with swapper,
    // tokenInputs, and tokenOutputs. This is far more reliable than
    // inferring from tokenBalanceChanges — which fails when userAccount
    // doesn't match the tracked wallet (common for newly-created token
    // ATAs where Helius reports the token account, not the owner).
    // Before this fix, ~98% of tracked-wallet SWAP events parsed as
    // Ok(None) because the speculative token's user_account didn't match.
    if let Some(swap_event) = &payload.events.swap {
        let in_mints: Vec<String> = swap_event.token_inputs.iter().map(|l| format!("{}({})", &l.mint[..l.mint.len().min(8)], l.raw_token_amount.is_some())).collect();
        let out_mints: Vec<String> = swap_event.token_outputs.iter().map(|l| format!("{}({})", &l.mint[..l.mint.len().min(8)], l.raw_token_amount.is_some())).collect();
        tracing::debug!(
            signature = %payload.signature,
            has_events_swap = true,
            token_inputs = swap_event.token_inputs.len(),
            token_outputs = swap_event.token_outputs.len(),
            in_mints = in_mints.join(","),
            out_mints = out_mints.join(","),
            "events.swap present — using primary parse path"
        );
        if let Some(parsed) = parse_from_swap_event(
            swap_event,
            &payload.signature,
            tracked_wallet,
            SOL_MINT,
            &STABLECOIN_MINTS,
        )? {
            return Ok(Some(parsed));
        }
    } else if tracked_wallet.is_some() {
        tracing::debug!(
            signature = %payload.signature,
            has_events_swap = false,
            "events.swap absent — falling back to tokenBalanceChanges"
        );
    }

    // Fallback: infer from token balance changes (original path).
    // When tracked_wallet is Some, only aggregate changes where user_account matches.
    // Helius Enhanced webhooks report net deltas, so multi-hop intermediate
    // tokens (e.g. USDC in SOL→USDC→TOKEN) correctly net to ~zero.
    let mut token_deltas: std::collections::HashMap<String, Decimal> =
        std::collections::HashMap::new();

    for account in &payload.account_data {
        if let Some(token_changes) = &account.token_balance_changes {
            for change in token_changes {
                // Filter by tracked wallet when provided
                if let Some(wallet) = tracked_wallet {
                    if change.user_account != wallet {
                        continue;
                    }
                }

                let amount = Decimal::from_str(&change.raw_token_amount.token_amount)
                    .unwrap_or(Decimal::ZERO);
                *token_deltas.entry(change.mint.clone()).or_insert(Decimal::ZERO) += amount;
            }
        }
    }

    // Find the traded speculative token (largest abs delta among non-SOL,
    // non-stablecoin mints) and the quote token (the stablecoin/SOL side).
    let mut traded_token: Option<(String, Decimal)> = None;
    let mut quote_mint: Option<String> = None;
    let mut quote_delta: Decimal = Decimal::ZERO;
    for (mint, delta) in &token_deltas {
        if delta.abs() <= Decimal::new(1, 6) {
            continue;
        }
        if mint == SOL_MINT || STABLECOIN_MINTS.contains(&mint.as_str()) {
            // Stablecoin/SOL side is the quote leg.
            if quote_delta.abs() < delta.abs() {
                quote_mint = Some(mint.clone());
                quote_delta = *delta;
            }
            continue;
        }
        // Speculative token: prefer the larger absolute delta.
        match &traded_token {
            Some((_, prev_delta)) if prev_delta.abs() >= delta.abs() => {}
            _ => traded_token = Some((mint.clone(), *delta)),
        }
    }

    let (token_mint, token_delta) = match traded_token {
        Some(v) => v,
        None => {
            // Diagnostic: 98% of tracked-wallet SWAP events return Ok(None)
            // here. Log the token_deltas map so we can see WHY no speculative
            // token was found (all SOL/stablecoin? amounts parsing as zero?
            // user_account filter removing all changes?).
            if tracked_wallet.is_some() && !token_deltas.is_empty() {
                let delta_summary: Vec<String> = token_deltas
                    .iter()
                    .map(|(m, d)| format!("{}={}", &m[..m.len().min(8)], d))
                    .collect();
                tracing::warn!(
                    signature = %payload.signature,
                    tracked_wallet = ?tracked_wallet,
                    token_deltas = delta_summary.join(", "),
                    "PARSE_FAIL: tracked wallet SWAP but no speculative token found — see deltas"
                );
            }
            return Ok(None); // no speculative token (pure stablecoin/dust swap)
        }
    };

    // Direction: positive token delta = received tokens = BUY; negative = SELL.
    let direction = if token_delta > Decimal::ZERO {
        SwapDirection::Buy
    } else {
        SwapDirection::Sell
    };

    // Compute the SOL leg amount. Prefer native_transfers (explicit SOL movements)
    // over native_balance_change (which includes fees and rent).
    let lamports_per_sol = Decimal::from(1_000_000_000u64);
    let mut sol_amount = Decimal::ZERO;

    // Sum absolute native transfer amounts — these are the explicit SOL legs of the swap.
    for transfer in &payload.native_transfers {
        sol_amount += Decimal::from(transfer.amount) / lamports_per_sol;
    }

    if sol_amount == Decimal::ZERO {
        // Fallback: use net native balance change if no explicit native transfers.
        let native_sol_delta: Decimal = payload
            .account_data
            .iter()
            .filter_map(|a| a.native_balance_change)
            .map(|c| Decimal::from(c) / lamports_per_sol)
            .sum();
        sol_amount = native_sol_delta.abs();
    }

    if sol_amount == Decimal::ZERO {
        // Last resort: token-to-token swap with no SOL leg — use token delta magnitude.
        sol_amount = token_delta.abs();
    }

    // Quote leg amount: stablecoin delta when USDC/USDT-quoted, else SOL amount.
    let quote_amount = match &quote_mint {
        Some(mint) if mint != SOL_MINT => quote_delta.abs(),
        _ => sol_amount,
    };
    let quote_mint = quote_mint.unwrap_or_else(|| SOL_MINT.to_string());

    // Detect DEX from webhook payload heuristics (best-effort; enriched webhooks
    // don't include instruction details, so this is approximate).
    let dex = detect_dex_from_payload(payload);

    Ok(Some(ParsedSwap {
        token_in: if direction == SwapDirection::Buy {
            quote_mint.clone()
        } else {
            token_mint.clone()
        },
        token_out: if direction == SwapDirection::Buy {
            token_mint
        } else {
            quote_mint
        },        amount_in: if direction == SwapDirection::Buy {
            quote_amount
        } else {
            token_delta.abs()
        },
        amount_out: if direction == SwapDirection::Buy {
            token_delta.abs()
        } else {
            quote_amount
        },
        direction,
        dex,
        slippage: None,
    }))
}

/// Best-effort DEX detection from Helius Enhanced webhook payload.
/// Enriched webhooks don't include program IDs, so we can only guess based on
/// the number of native transfers and token deltas. Returns "Unknown" if uncertain.
fn detect_dex_from_payload(payload: &crate::monitoring::helius::HeliusWebhookPayload) -> String {
    // Heuristic: Jupiter routes typically have 2+ native transfers and multiple
    // token balance changes (intermediate hops). Direct DEX swaps usually have
    // exactly 2 native transfers and 1-2 token changes.
    let native_transfer_count = payload.native_transfers.len();
    let token_change_count = payload
        .account_data
        .iter()
        .filter_map(|a| a.token_balance_changes.as_ref().map(|c| c.len()))
        .sum::<usize>();

    if token_change_count > 2 || native_transfer_count > 4 {
        "Jupiter".to_string() // Multi-hop route — likely Jupiter aggregator
    } else {
        "Unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::helius::{
        AccountData, HeliusWebhookPayload, NativeTransfer, RawTokenAmount, TokenBalanceChange,
    };

    const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

    fn make_payload(
        account_data: Vec<AccountData>,
        native_transfers: Vec<NativeTransfer>,
    ) -> HeliusWebhookPayload {
        HeliusWebhookPayload {
            account_data,
            native_transfers,
            signature: "sig_test".to_string(),
            slot: 1,
            timestamp: 1,
            transaction_error: None,
            transaction_type: "SWAP".to_string(),
            events: Default::default(),
        }
    }

    fn token_change(mint: &str, amount: &str, account: &str) -> TokenBalanceChange {
        TokenBalanceChange {
            mint: mint.to_string(),
            raw_token_amount: RawTokenAmount {
                token_amount: amount.to_string(),
                decimals: None,
            },
            token_account: format!("{}acct", account),
            user_account: account.to_string(),
        }
    }

    #[test]
    fn test_simple_buy_spend_sol_receive_token() {
        // Buy: wallet spends 1 SOL, receives 1000 tokens
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: Some(-1_000_000_000), // -1 SOL
                token_balance_changes: Some(vec![token_change(token, "1000", wallet)]),
            }],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse");
        assert_eq!(swap.direction, SwapDirection::Buy);
        assert_eq!(swap.token_out, token);
        assert_eq!(swap.token_in, SOL_MINT);
        assert_eq!(swap.amount_out, rust_decimal::Decimal::new(1000, 0));
    }

    #[test]
    fn test_simple_sell_receive_sol_lose_token() {
        // Sell: wallet loses 500 tokens, receives 0.5 SOL
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: Some(500_000_000), // +0.5 SOL
                token_balance_changes: Some(vec![token_change(token, "-500", wallet)]),
            }],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse");
        assert_eq!(swap.direction, SwapDirection::Sell);
        assert_eq!(swap.token_in, token);
        assert_eq!(swap.token_out, SOL_MINT);
    }

    #[test]
    fn test_multihop_intermediate_token_nets_to_zero() {
        // SOL -> USDC -> TARGET_TOKEN multi-hop.
        // USDC nets to zero (intermediate hop); only TARGET should be detected.
        let wallet = "Wallet111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let target = "Target1111111111111111111111111111111111111";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: Some(-1_000_000_000),
                token_balance_changes: Some(vec![
                    token_change(usdc, "2000", wallet),  // receive USDC
                    token_change(usdc, "-2000", wallet), // spend USDC (nets to 0)
                    token_change(target, "5000", wallet), // receive target
                ]),
            }],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse");
        assert_eq!(swap.direction, SwapDirection::Buy);
        assert_eq!(swap.token_out, target, "must pick target, not intermediate USDC");
    }

    #[test]
    fn test_sol_amount_from_native_transfers() {
        // native_transfers should be preferred over native_balance_change
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: Some(-50_000_000), // small change (rent/fee)
                token_balance_changes: Some(vec![token_change(token, "100", wallet)]),
            }],
            vec![NativeTransfer {
                amount: 2_000_000_000, // 2 SOL actual swap
                from_user_account: wallet.to_string(),
                to_user_account: "DexRouter111111111111111111111111111111111".to_string(),
            }],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse");
        // Should use native_transfers (2 SOL), not native_balance_change (0.05 SOL)
        assert!(
            swap.amount_in >= rust_decimal::Decimal::new(2, 0),
            "expected SOL amount from native_transfers, got {}",
            swap.amount_in
        );
    }

    #[test]
    fn test_non_swap_transaction_returns_none() {
        let payload = HeliusWebhookPayload {
            account_data: vec![],
            native_transfers: vec![],
            signature: "sig".to_string(),
            slot: 1,
            timestamp: 1,
            transaction_error: None,
            transaction_type: "TRANSFER".to_string(),
            events: Default::default(),
        };
        assert!(parse_helius_webhook(&payload, None).unwrap().is_none());
    }

    #[test]
    fn test_sol_only_transaction_returns_none() {
        // Pure SOL transfer, no token changes — not a swap we care about
        let wallet = "Wallet111111111111111111111111111111111111";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: Some(-1_000_000_000),
                token_balance_changes: None,
            }],
            vec![],
        );
        assert!(parse_helius_webhook(&payload, Some(wallet)).unwrap().is_none());
    }

    #[test]
    fn test_webhook_direct_sell_net_zero_without_filter() {
        // Reproduce the production bug: tracked wallet SELLing token, DEX receiving same amount.
        // Without filtering, the net delta is 0, so parser returns None.
        let wallet = "DakNYZdrGeFwXYZXYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let dex = "DhTZ9VELL65GXYZXYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let token = "FeVAWnmq9PToqEW6XYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let amount = "3414264284053";

        let payload = make_payload(
            vec![
                AccountData {
                    account: wallet.to_string(),
                    native_balance_change: Some(500_000_000), // +0.5 SOL received
                    token_balance_changes: Some(vec![token_change(token, &format!("-{}", amount), wallet)]),
                },
                AccountData {
                    account: dex.to_string(),
                    native_balance_change: Some(-500_000_000), // -0.5 SOL sent
                    token_balance_changes: Some(vec![token_change(token, amount, dex)]),
                },
            ],
            vec![],
        );

        // Without wallet filter: net delta is 0, parser returns None
        assert!(parse_helius_webhook(&payload, None).unwrap().is_none());
    }

    #[test]
    fn test_webhook_direct_sell_with_tracked_wallet() {
        // Same payload as above, but with wallet filter applied.
        // Parser should correctly detect the SELL.
        let wallet = "DakNYZdrGeFwXYZXYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let dex = "DhTZ9VELL65GXYZXYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let token = "FeVAWnmq9PToqEW6XYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let amount = "3414264284053";

        let payload = make_payload(
            vec![
                AccountData {
                    account: wallet.to_string(),
                    native_balance_change: Some(500_000_000), // +0.5 SOL received
                    token_balance_changes: Some(vec![token_change(token, &format!("-{}", amount), wallet)]),
                },
                AccountData {
                    account: dex.to_string(),
                    native_balance_change: Some(-500_000_000), // -0.5 SOL sent
                    token_balance_changes: Some(vec![token_change(token, amount, dex)]),
                },
            ],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse with wallet filter");
        assert_eq!(swap.direction, SwapDirection::Sell);
        assert_eq!(swap.token_in, token);
        assert_eq!(swap.token_out, SOL_MINT);
        assert_eq!(swap.amount_out, Decimal::from_str(amount).unwrap());
    }

    #[test]
    fn test_webhook_buy_with_tracked_wallet() {
        // BUY: tracked wallet receives tokens (positive delta)
        let wallet = "DakNYZdrGeFwXYZXYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let dex = "DhTZ9VELL65GXYZXYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let token = "FeVAWnmq9PToqEW6XYZXYZXYZXYZXYZXYZXYZXYZXYZ";
        let amount = "1000000000";

        let payload = make_payload(
            vec![
                AccountData {
                    account: wallet.to_string(),
                    native_balance_change: Some(-500_000_000), // -0.5 SOL sent
                    token_balance_changes: Some(vec![token_change(token, amount, wallet)]),
                },
                AccountData {
                    account: dex.to_string(),
                    native_balance_change: Some(500_000_000), // +0.5 SOL received
                    token_balance_changes: Some(vec![token_change(token, &format!("-{}", amount), dex)]),
                },
            ],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse with wallet filter");
        assert_eq!(swap.direction, SwapDirection::Buy);
        assert_eq!(swap.token_in, SOL_MINT);
        assert_eq!(swap.token_out, token);
        assert_eq!(swap.amount_out, Decimal::from_str(amount).unwrap());
    }

    #[test]
    fn test_webhook_usdc_quoted_buy_picks_token_not_usdc() {
        // USDC-quoted BUY: wallet spends USDC, receives TOKEN.
        // The parser must resolve the speculative TOKEN (not USDC) as the
        // traded token and label it a Buy — previously this became
        // "Sell USDC" (NO_ACTIVE_POSITION) and the signal was lost.
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        // USDC has the LARGER absolute delta (decimal 6 vs token decimal 9)
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: None,
                token_balance_changes: Some(vec![
                    token_change(usdc, "-1000000", wallet),  // -1 USDC (6 decimals)
                    token_change(token, "1000000000000", wallet), // +1M tokens
                ]),
            }],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse");
        assert_eq!(swap.direction, SwapDirection::Buy);
        assert_eq!(swap.token_in, usdc);
        assert_eq!(swap.token_out, token);
    }

    #[test]
    fn test_webhook_usdc_quoted_sell_picks_token_not_usdc() {
        // USDC-quoted SELL: wallet spends TOKEN, receives USDC.
        let wallet = "Wallet111111111111111111111111111111111111";
        let token = "TokA111111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: None,
                token_balance_changes: Some(vec![
                    token_change(usdc, "2000000", wallet),        // +2 USDC
                    token_change(token, "-1000000000000", wallet), // -1M tokens
                ]),
            }],
            vec![],
        );

        let swap = parse_helius_webhook(&payload, Some(wallet)).unwrap().expect("should parse");
        assert_eq!(swap.direction, SwapDirection::Sell);
        assert_eq!(swap.token_in, token);
        assert_eq!(swap.token_out, usdc);
    }

    #[test]
    fn test_webhook_sol_to_usdc_dust_returns_none() {
        // SOL -> USDC swap has no speculative token: must return None
        // (previously produced "Buy USDC" NON_SPECULATIVE noise).
        let wallet = "Wallet111111111111111111111111111111111111";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let payload = make_payload(
            vec![AccountData {
                account: wallet.to_string(),
                native_balance_change: Some(-100_000_000), // -0.1 SOL
                token_balance_changes: Some(vec![token_change(usdc, "100000", wallet)]),
            }],
            vec![],
        );

        assert!(parse_helius_webhook(&payload, Some(wallet)).unwrap().is_none());
    }
}
