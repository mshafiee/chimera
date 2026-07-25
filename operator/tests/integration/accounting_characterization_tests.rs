//! Accounting Characterization Tests (A1/A2 semantics)
//!
//! These tests PIN the canonical accounting contract introduced by the
//! profitability remediation:
//!
//! 1. Multi-tier partial + final close ACCUMULATES realized PnL (final close
//!    must never overwrite earlier tranches).
//! 2. Fallback PnL branches are dimensionally consistent: return-times-capital
//!    in every branch (never per-token diff / SOL-price without exposure).
//! 3. Attribution-only cost model: dex_fee_sol / slippage_cost_sol columns are
//!    recorded but NEVER subtracted from net PnL (embedded in executable
//!    fills); only tips + network fees are deducted.
//! 4. Exit-side costs are taken from the SELL trade row (distinct UUID), not
//!    from the entry trade.
//! 5. Atomic portfolio admission: concurrent same-token opens leave exactly
//!    one ACTIVE position.

use chimera_operator::db_abstraction::{Database, InsertTrade};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    crate::common::pg_pool(db)
}

async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    crate::common::create_test_pg_db().await
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn insert_trade(
    db: &Arc<dyn Database>,
    uuid: &str,
    wallet: &str,
    token: &str,
    side: &str,
    amount: &str,
) {
    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: wallet.to_string(),
        token_address: token.to_string(),
        token_symbol: Some("T1".to_string()),
        strategy: "SHIELD".to_string(),
        side: side.to_string(),
        amount_sol: dec(amount),
        status: "PENDING".to_string(),
    })
    .await
    .unwrap();
}

async fn set_trade_costs(
    pool: &Pool<Postgres>,
    uuid: &str,
    tip: &str,
    dex: &str,
    slip: &str,
    nf: &str,
) {
    sqlx::query(
        "UPDATE trades SET jito_tip_sol = $1, dex_fee_sol = $2, slippage_cost_sol = $3, network_fee_sol = $4 WHERE trade_uuid = $5",
    )
    .bind(dec(tip))
    .bind(dec(dex))
    .bind(dec(slip))
    .bind(dec(nf))
    .bind(uuid)
    .execute(pool)
    .await
    .unwrap();
}

async fn open_position(
    db: &Arc<dyn Database>,
    uuid: &str,
    wallet: &str,
    token: &str,
    amount: &str,
    entry_price: &str,
    entry_sol_price: Option<Decimal>,
) {
    insert_trade(db, uuid, wallet, token, "BUY", amount).await;
    db.activate_trade_and_open_position(
        uuid,
        wallet,
        token,
        Some("T1"),
        "SHIELD",
        dec(amount),
        dec(entry_price),
        "sig_entry",
        None,
        entry_sol_price,
    )
    .await
    .unwrap();
}

async fn position_row(
    pool: &Pool<Postgres>,
    uuid: &str,
) -> (String, Decimal, Option<Decimal>, Decimal) {
    let (state, realized, realized_net, remaining): (String, Decimal, Option<Decimal>, Decimal) =
        sqlx::query_as(
            "SELECT state, COALESCE(realized_pnl_sol,0), realized_net_pnl_sol, entry_amount_sol FROM positions WHERE trade_uuid = $1",
        )
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    (state, realized, realized_net, remaining)
}

// ─── 1. Multi-tier partial + final close accumulates realized PnL ─────────────

#[tokio::test]
async fn test_tiered_close_accumulates_realized_pnl() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    const W: &str = "w-tier";
    const T: &str = "tok-tier";

    // Entry: 1.0 SOL at $0.001/token, SOL/USD = 100.
    open_position(&db, "uuid-tier", W, T, "1.0", "0.001", Some(dec("100"))).await;
    // Entry costs: tip 0.001 + nf 0.000005 (dex/slippage present but attribution-only).
    set_trade_costs(&pool, "uuid-tier", "0.001", "0.01", "0.01", "0.000005").await;

    // Tier 1: close 50% at $0.002 (ratio = 2.0 → gross = 0.5 × 1.0 = 0.5 SOL).
    insert_trade(&db, "uuid-tier-exit1", W, T, "SELL", "0.5").await;
    db.close_position_full(
        "uuid-tier-exit1",
        W,
        T,
        dec("0.002"),
        "sig_e1",
        Some(dec("100")),
        dec("0.5"),
        true,
    )
    .await
    .unwrap();

    let (state, realized, realized_net, remaining) = position_row(&pool, "uuid-tier").await;
    assert_eq!(state, "ACTIVE");
    assert_eq!(remaining, dec("0.5"), "half the position must remain");
    assert!(
        (realized - dec("0.5")).abs() < dec("0.000001"),
        "tier-1 gross must be 0.5 SOL, got {}",
        realized
    );
    // tier-1 net = 0.5 − prop(tip 0.001×0.5) − prop(nf 0.000005×0.5) = 0.4994975
    let expected_net1 = dec("0.4994975");
    let net1 = realized_net.unwrap_or(Decimal::ZERO);
    assert!(
        (net1 - expected_net1).abs() < dec("0.0000001"),
        "tier-1 net must be {} (tips+nf only), got {}",
        expected_net1,
        net1
    );

    // Tier 2 (final): close remaining at $0.003 (ratio = 3.0 → gross = 0.5 × 2.0 = 1.0 SOL).
    insert_trade(&db, "uuid-tier-exit2", W, T, "SELL", "0.5").await;
    db.close_position_full(
        "uuid-tier-exit2",
        W,
        T,
        dec("0.003"),
        "sig_e2",
        Some(dec("100")),
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    let (state, realized, realized_net, _remaining) = position_row(&pool, "uuid-tier").await;
    assert_eq!(state, "CLOSED");
    assert!(
        (realized - dec("1.5")).abs() < dec("0.000001"),
        "final close must ACCUMULATE: 0.5 + 1.0 = 1.5 SOL gross, got {}",
        realized
    );
    // tier-2 net = 1.0 − 0.0005025 = 0.9994975; total = 1.4989950
    let expected_total_net = dec("1.498995");
    let total_net = realized_net.unwrap_or(Decimal::ZERO);
    assert!(
        (total_net - expected_total_net).abs() < dec("0.000001"),
        "cumulative net must be {}, got {}",
        expected_total_net,
        total_net
    );
}

// ─── 2. Attribution-only cost model: dex/slippage never subtracted ────────────

#[tokio::test]
async fn test_dex_and_slippage_are_attribution_only() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    const W: &str = "w-attr";
    const T: &str = "tok-attr";

    open_position(&db, "uuid-attr", W, T, "1.0", "0.001", Some(dec("100"))).await;
    // Large dex_fee/slippage values: if these were subtracted, net would go
    // deeply negative. Only tip 0.001 + nf 0.000005 are real costs.
    set_trade_costs(&pool, "uuid-attr", "0.001", "0.05", "0.05", "0.000005").await;

    insert_trade(&db, "uuid-attr-exit", W, T, "SELL", "1.0").await;
    db.close_position_full(
        "uuid-attr-exit",
        W,
        T,
        dec("0.0011"), // +10% → gross 0.1 SOL
        "sig_e",
        Some(dec("100")),
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    let (_, _, realized_net, _) = position_row(&pool, "uuid-attr").await;
    // net = 0.1 − 0.001 − 0.000005 = 0.098995 (NOT 0.1 − 0.001 − 0.05 − 0.05 − …)
    let expected = dec("0.098995");
    let net = realized_net.unwrap_or(Decimal::ZERO);
    assert!(
        (net - expected).abs() < dec("0.000001"),
        "net must exclude dex/slippage attribution columns: expected {}, got {}",
        expected,
        net
    );
}

// ─── 3. Fallback branch dimensional invariant (SOL price present at entry only) ─

#[tokio::test]
async fn test_fallback_pnl_branch_is_return_times_capital() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    const W: &str = "w-fb";
    const T: &str = "tok-fb";

    // Entry WITH entry_sol_price_usd = 100; close passes sol_price_usd = None.
    open_position(&db, "uuid-fb", W, T, "1.0", "0.001", Some(dec("100"))).await;

    insert_trade(&db, "uuid-fb-exit", W, T, "SELL", "1.0").await;
    db.close_position_full(
        "uuid-fb-exit",
        W,
        T,
        dec("0.0015"), // +50% in USD terms
        "sig_e",
        None, // SOL/USD unavailable at close → fallback branch
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    let (_, realized, _, _) = position_row(&pool, "uuid-fb").await;
    // Correct: (0.0015/0.001 − 1) × 1.0 = 0.5 SOL.
    // Old bug: usd_diff / entry_sol_price = 0.0005/100 = 0.000005 SOL.
    assert!(
        (realized - dec("0.5")).abs() < dec("0.000001"),
        "fallback branch must be return-times-capital (0.5 SOL), got {}",
        realized
    );
}

// ─── 4. Exit-side costs come from the SELL trade row ──────────────────────────

#[tokio::test]
async fn test_exit_costs_come_from_sell_trade_row() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    const W: &str = "w-excost";
    const T: &str = "tok-excost";

    open_position(&db, "uuid-excost", W, T, "1.0", "0.001", Some(dec("100"))).await;
    set_trade_costs(&pool, "uuid-excost", "0.001", "0", "0", "0.000005").await;

    insert_trade(&db, "uuid-excost-exit", W, T, "SELL", "1.0").await;
    // SELL-side costs: tip 0.002 + nf 0.00001.
    set_trade_costs(&pool, "uuid-excost-exit", "0.002", "0", "0", "0.00001").await;

    db.close_position_full(
        "uuid-excost-exit",
        W,
        T,
        dec("0.0011"),
        "sig_e",
        Some(dec("100")),
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    let (_, _, realized_net, _) = position_row(&pool, "uuid-excost").await;
    // net = 0.1 (gross) − 0.001 (entry tip) − 0.000005 (entry nf)
    //       − 0.002 (exit tip) − 0.00001 (exit nf) = 0.096985
    let expected = dec("0.096985");
    let net = realized_net.unwrap_or(Decimal::ZERO);
    assert!(
        (net - expected).abs() < dec("0.000001"),
        "net must include exit-row tip + network fee: expected {}, got {}",
        expected,
        net
    );

    // Aggregate trade-level net on the SELL row must equal position net.
    let trade_net: Option<Decimal> =
        sqlx::query_scalar("SELECT net_pnl_sol FROM trades WHERE trade_uuid = $1")
            .bind("uuid-excost-exit")
            .fetch_one(&pool)
            .await
            .unwrap();
    let trade_net = trade_net.unwrap_or(Decimal::ZERO);
    assert!(
        (trade_net - expected).abs() < dec("0.000001"),
        "trade net must match position net: expected {}, got {}",
        expected,
        trade_net
    );
}

// ─── 5. Atomic admission: concurrent same-token opens leave one ACTIVE position ─

#[tokio::test]
async fn test_concurrent_same_token_open_leaves_one_position() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    const W: &str = "w-race";
    const T: &str = "tok-race";

    // Pre-insert distinct trade rows (as the signal receipt path would).
    for i in 0..5 {
        insert_trade(&db, &format!("uuid-race-{}", i), W, T, "BUY", "1.0").await;
    }

    let mut handles = Vec::new();
    for i in 0..5 {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            db.atomic_portfolio_heat_check_and_open_position(
                &format!("uuid-race-{}", i),
                W,
                T,
                Some("T1"),
                "SHIELD",
                dec("1.0"),
                dec("0.001"),
                &format!("sig-{}", i),
                None,
                Some(dec("100")),
            )
            .await
        }));
    }

    let mut successes = 0;
    for h in handles {
        if h.await.unwrap().is_ok() {
            successes += 1;
        }
    }
    assert_eq!(successes, 1, "exactly one concurrent open may succeed");

    let (active,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM positions WHERE wallet_address = $1 AND token_address = $2 AND state = 'ACTIVE'",
    )
    .bind(W)
    .bind(T)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 1, "exactly one ACTIVE position must exist");
}
