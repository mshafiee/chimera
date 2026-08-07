//! `bootstrap_dune` — one-shot Dune bootstrap: wallet PnL history + roster seed.
//!
//! Bootstraps the trading system with historical evidence from Dune Analytics
//! so the wallet-level gates (t-stat, proven-wallet, smart-money cluster) work
//! from day one instead of cold-starting on live shadow data:
//!
//! 1. **Wallet PnL history** — per-round-trip pnl rows per wallet, written to
//!    `shadow_positions`/`shadow_exits` with `exit_strategy='dune_wallet'`,
//!    `shadow_id` prefix `dune_`, `exit_reason='dune_bootstrap'`. These rows
//!    feed `get_wallet_pnl_statistics` (which unions `mirror_main` +
//!    `dune_wallet`); the token mirror gate reads ONLY `mirror_main`, so the
//!    bootstrap rows can never count as mirror-gate evidence.
//! 2. **Roster refresh** (`--roster`) — backfill the `wallets` table with the
//!    bootstrap wallets as CANDIDATE (seed-only; status changes stay owned by
//!    the live DunePnlMonitor promote cycle).
//!
//! Usage (runs on the server via `docker compose run`):
//! ```text
//! bootstrap_dune                        # dry-run (default; no writes)
//! bootstrap_dune --apply                # real run: replace dune_% rows, idempotent
//! bootstrap_dune --apply --roster       # + seed wallets roster (CANDIDATE only)
//! bootstrap_dune --apply --no-roster    # skip roster even if config enables it
//! bootstrap_dune --wallet <addr>        # process only this wallet
//! bootstrap_dune --create-query         # create the query via the Dune API when
//!                                       # dune.bootstrap_query_id is 0, then run
//! ```
//!
//! `--create-query` copies the trades table from an existing workspace query
//! (default: `dune.promote_query_id`; override with `--from-query <id>`),
//! generates the per-wallet round-trip PnL query, and creates it privately via
//! `POST /api/v1/query`. The new query ID is logged — set it in
//! `config/config.yaml` (`dune.bootstrap_query_id`) so subsequent runs reuse
//! it instead of creating a new query each time.
//!
//! Fail-closed: any Dune API error or result parse error → log + exit nonzero,
//! no writes. Idempotent: re-runs delete the wallet's `dune_%` rows before
//! re-inserting (the `UNIQUE(shadow_id, exit_strategy)` constraint would
//! otherwise collide on re-run).

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{
    create_database, Database, DatabaseBackend, DatabaseConfig, DbPool,
};
use chimera_operator::utils::is_dev_mode;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use serde::Deserialize;
use tracing::{error, info, warn};

const DUNE_API_BASE: &str = "https://api.dune.com/api/v1";
const POLL_INTERVAL_SECS: u64 = 10;
const MAX_POLLS: usize = 60;
const RESULTS_PAGE_LIMIT: usize = 1000;
const MAX_RESULT_PAGES: usize = 40;

const EXIT_STRATEGY: &str = "dune_wallet";
const EXIT_REASON: &str = "dune_bootstrap";
const ENTRY_AMOUNT_SOL: &str = "0.1";

/// Mirrors `SelectionConfig` defaults (wallet_tstat_threshold /
/// wallet_tstat_min_samples) for the dry-run verdict report. The live gates
/// read their own config; these constants only label the report.
const TSTAT_THRESHOLD: f64 = 1.645;
const TSTAT_MIN_SAMPLES: i64 = 10;

#[derive(Deserialize)]
struct ExecutionResponse {
    execution_id: String,
}

#[derive(Deserialize)]
struct StatusResponse {
    state: String,
    #[serde(default)]
    error: Option<DuneError>,
}

#[derive(Deserialize)]
struct DuneError {
    message: String,
}

/// One round trip from the Dune bootstrap query.
#[derive(Debug, Clone)]
struct BootstrapTrade {
    wallet_address: String,
    token_address: String,
    pnl_pct: Decimal,
    exit_ts: DateTime<Utc>,
    hold_duration_secs: Option<i64>,
}

/// Per-wallet aggregate for the report + roster.
#[derive(Debug, Clone)]
struct WalletSummary {
    address: String,
    trades: Vec<BootstrapTrade>,
}

impl WalletSummary {
    fn mean_pnl_pct(&self) -> Decimal {
        if self.trades.is_empty() {
            return Decimal::ZERO;
        }
        self.trades
            .iter()
            .fold(Decimal::ZERO, |acc, t| acc + t.pnl_pct)
            / Decimal::from(self.trades.len())
    }

    /// Sample stddev (n-1 denominator — same as postgres STDDEV, which is what
    /// `get_wallet_pnl_statistics` returns).
    fn stddev_pnl_pct(&self) -> Decimal {
        let n = self.trades.len();
        if n < 2 {
            return Decimal::ZERO;
        }
        let mean = self.mean_pnl_pct();
        let sum_sq = self.trades.iter().fold(Decimal::ZERO, |acc, t| {
            let d = t.pnl_pct - mean;
            acc + d * d
        });
        (sum_sq / Decimal::from(n - 1))
            .sqrt()
            .unwrap_or(Decimal::ZERO)
    }

    fn t_stat(&self) -> f64 {
        let n = self.trades.len() as f64;
        let mean = self.mean_pnl_pct().to_f64().unwrap_or(0.0);
        let stddev = self.stddev_pnl_pct().to_f64().unwrap_or(0.0);
        if stddev <= 0.0 {
            // Zero variance with positive mean = perfectly consistent.
            return if mean > 0.0 { f64::INFINITY } else { 0.0 };
        }
        let se = stddev / n.sqrt();
        mean / se
    }

    fn win_rate(&self) -> f64 {
        if self.trades.is_empty() {
            return 0.0;
        }
        let wins = self
            .trades
            .iter()
            .filter(|t| t.pnl_pct > Decimal::ZERO)
            .count();
        wins as f64 / self.trades.len() as f64
    }

    fn passes_tstat(&self) -> bool {
        self.trades.len() as i64 >= TSTAT_MIN_SAMPLES
            && self.mean_pnl_pct() > Decimal::ZERO
            && self.t_stat() > TSTAT_THRESHOLD
    }

    /// Low-confidence WQS heuristic for roster seeding. Documented placeholder:
    /// the DunePnlMonitor promote cycle (6h) audits CANDIDATE wallets on-chain
    /// and overrides with real metrics before ACTIVE promotion.
    fn wqs_heuristic(&self) -> f64 {
        let wqs = 40.0 + self.win_rate() * 60.0;
        wqs.min(99.0)
    }
}

struct Args {
    dry_run: bool,
    apply: bool,
    roster: bool,
    no_roster: bool,
    wallet: Option<String>,
    /// Create the bootstrap query via the Dune API (POST /api/v1/query) when
    /// `dune.bootstrap_query_id` is 0, then use the new query ID.
    create_query: bool,
    /// Reference query to copy the trades table from (default:
    /// `dune.promote_query_id`).
    from_query: Option<u64>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        dry_run: true,
        apply: false,
        roster: false,
        no_roster: false,
        wallet: None,
        create_query: false,
        from_query: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" => {
                if args.apply {
                    bail!("--dry-run and --apply are mutually exclusive");
                }
                args.dry_run = true;
            }
            "--apply" => {
                if args.dry_run {
                    args.dry_run = false;
                }
                args.apply = true;
            }
            "--roster" => args.roster = true,
            "--no-roster" => args.no_roster = true,
            "--create-query" => args.create_query = true,
            "--from-query" => {
                let id = it
                    .next()
                    .ok_or_else(|| anyhow!("--from-query requires a query ID argument"))?;
                args.from_query = Some(id.parse::<u64>().map_err(|_| {
                    anyhow!("--from-query expects a numeric Dune query ID, got '{id}'")
                })?);
            }
            "--wallet" => {
                let addr = it
                    .next()
                    .ok_or_else(|| anyhow!("--wallet requires an address argument"))?;
                if addr.len() < 32 {
                    bail!("--wallet address too short: {addr}");
                }
                args.wallet = Some(addr);
            }
            "--help" | "-h" => {
                println!(
                    "bootstrap_dune — one-shot Dune wallet PnL history + roster seed\n\
                     \n\
                     USAGE:\n\
                     \x20 bootstrap_dune [--dry-run] [--apply] [--roster] [--no-roster] [--wallet <addr>]\n\
                     \x20               [--create-query] [--from-query <id>]\n\
                     \n\
                     \x20 --dry-run        report only, no writes (default)\n\
                     \x20 --apply          real run: replace dune_% rows, idempotent\n\
                     \x20 --roster         also seed the wallets roster as CANDIDATE\n\
                     \x20 --no-roster      skip roster seeding even if config enables it\n\
                     \x20 --wallet         process only this wallet\n\
                     \x20 --create-query   create the bootstrap query via the Dune API when\n\
                     \x20                   dune.bootstrap_query_id is 0, then run with it\n\
                     \x20 --from-query <id> reference query to copy the trades table from\n\
                     \x20                   (default: dune.promote_query_id)\n"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other} (see --help)"),
        }
    }
    Ok(args)
}

fn init_tracing() {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Same load_config path as api/src/main.rs: .env, dev-mode guard, validate.
fn load_config() -> Result<AppConfig> {
    dotenvy::dotenv().ok();

    if is_dev_mode() && std::env::var("CHIMERA_ENV").as_deref() == Ok("production") {
        bail!(
            "CHIMERA_DEV_MODE is set in a production environment (CHIMERA_ENV=production). \
             Unset CHIMERA_DEV_MODE before running."
        );
    }

    let config = AppConfig::load_config().map_err(|e| anyhow!("Configuration error: {e}"))?;

    if let Err(e) = config.validate() {
        if is_dev_mode() {
            warn!("Running in dev mode - skipping configuration validation");
        } else {
            return Err(anyhow!("Configuration validation failed: {e}"));
        }
    }

    Ok(config)
}

struct DuneClient {
    api_key: String,
    http: reqwest::Client,
}

impl DuneClient {
    fn new(api_key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { api_key, http })
    }

    async fn execute_query(&self, query_id: u64) -> Result<String> {
        let url = format!("{DUNE_API_BASE}/query/{query_id}/execute");
        let resp = self
            .http
            .post(&url)
            .header("X-Dune-Api-Key", &self.api_key)
            .send()
            .await
            .context("Dune execute request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Dune execute returned HTTP {status}: {body}");
        }
        let exec: ExecutionResponse = resp
            .json()
            .await
            .context("Dune execute response parse failed")?;
        Ok(exec.execution_id)
    }

    /// Poll until QUERY_STATE_COMPLETED; fail-closed on QUERY_STATE_FAILED.
    async fn wait_for_completion(&self, execution_id: &str) -> Result<()> {
        let status_url = format!("{DUNE_API_BASE}/execution/{execution_id}/status");
        for _ in 0..MAX_POLLS {
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
            let resp = self
                .http
                .get(&status_url)
                .header("X-Dune-Api-Key", &self.api_key)
                .send()
                .await
                .context("Dune status request failed")?;
            let status: StatusResponse = resp
                .json()
                .await
                .context("Dune status response parse failed")?;
            match status.state.as_str() {
                "QUERY_STATE_COMPLETED" => return Ok(()),
                "QUERY_STATE_FAILED" => {
                    let msg = status
                        .error
                        .map(|e| e.message)
                        .unwrap_or_else(|| "unknown error".to_string());
                    bail!("Dune query failed: {msg}");
                }
                _ => { /* still pending/executing */ }
            }
        }
        bail!("Dune query timed out after {MAX_POLLS} polls");
    }

    /// Fetch JSON result rows, paginated (defensive: ~50 trades × 60 wallets
    /// can exceed a single response). Accepts both the Dune v1 shape
    /// (`{"result": {"rows": [...]}}` used by the monitor) and the native
    /// flat shape (`{"rows": [...]}`).
    async fn fetch_result_rows(&self, execution_id: &str) -> Result<Vec<serde_json::Value>> {
        let mut rows = Vec::new();
        let mut offset = 0usize;
        loop {
            let url = format!(
                "{DUNE_API_BASE}/execution/{execution_id}/results?limit={RESULTS_PAGE_LIMIT}&offset={offset}"
            );
            let resp = self
                .http
                .get(&url)
                .header("X-Dune-Api-Key", &self.api_key)
                .send()
                .await
                .context("Dune results request failed")?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                bail!("Dune results returned HTTP {status}: {body}");
            }
            let body: serde_json::Value = resp
                .json()
                .await
                .context("Dune results response parse failed")?;
            let page: Vec<serde_json::Value> = body
                .get("result")
                .and_then(|r| r.get("rows"))
                .or_else(|| body.get("rows"))
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            let page_len = page.len();
            rows.extend(page);
            offset += page_len;
            if page_len < RESULTS_PAGE_LIMIT {
                break;
            }
            if rows.len() >= RESULTS_PAGE_LIMIT * MAX_RESULT_PAGES {
                bail!(
                    "Dune results exceed {MAX_RESULT_PAGES} pages ({} rows) — aborting fail-closed",
                    rows.len()
                );
            }
        }
        Ok(rows)
    }

    /// Fetch a query's SQL (for the table-name discovery behind
    /// `--create-query`).
    async fn fetch_query_sql(&self, query_id: u64) -> Result<String> {
        let url = format!("{DUNE_API_BASE}/query/{query_id}");
        let resp = self
            .http
            .get(&url)
            .header("X-Dune-Api-Key", &self.api_key)
            .send()
            .await
            .context("Dune query fetch request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Dune query fetch returned HTTP {status}: {body}");
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .context("Dune query fetch response parse failed")?;
        let sql = body
            .get("query_sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Dune query response has no query_sql"))?;
        Ok(sql.to_string())
    }

    /// Create a private query via `POST /api/v1/query`, returning its ID.
    async fn create_query(&self, name: &str, query_sql: &str) -> Result<u64> {
        let url = format!("{DUNE_API_BASE}/query");
        let body = serde_json::json!({
            "name": name,
            "query_sql": query_sql,
            "is_private": true,
        });
        let resp = self
            .http
            .post(&url)
            .header("X-Dune-Api-Key", &self.api_key)
            .json(&body)
            .send()
            .await
            .context("Dune query create request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Dune query create returned HTTP {status}: {text}");
        }
        let created: serde_json::Value = resp
            .json()
            .await
            .context("Dune query create response parse failed")?;
        let query_id = created
            .get("query_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("Dune query create response has no query_id"))?;
        Ok(query_id)
    }

    async fn fetch_bootstrap_trades(&self, query_id: u64) -> Result<Vec<BootstrapTrade>> {
        let execution_id = self.execute_query(query_id).await?;
        info!(execution_id = %execution_id, "Dune query execution started");
        self.wait_for_completion(&execution_id).await?;
        let rows = self.fetch_result_rows(&execution_id).await?;
        info!(rows = rows.len(), "Dune query completed");

        // Fail-closed on any malformed row: report the first error + count.
        let mut trades = Vec::with_capacity(rows.len());
        let mut error_count = 0usize;
        let mut first_errors = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            match parse_trade(row) {
                Ok(t) => trades.push(t),
                Err(e) => {
                    error_count += 1;
                    if first_errors.len() < 5 {
                        first_errors.push(format!("row {i}: {e}"));
                    }
                }
            }
        }
        if error_count > 0 {
            bail!(
                "{error_count} of {} rows failed to parse (first: {}) — aborting fail-closed, no writes",
                rows.len(),
                first_errors.join("; ")
            );
        }
        Ok(trades)
    }
}

/// Reference bootstrap query template. `{trades_table}` is substituted with
/// the table discovered from the workspace's existing Dune queries (e.g.
/// `dex_solana.trades`) — the standard DEX trades dataset: one row per swap
/// fill with `trader_id`, `token_*_mint_address`, `token_*_symbol`,
/// `amount_usd`, `block_time`, `project`.
///
/// Round-trip construction: pair each stable-denominated buy of a token with
/// the nth stable-denominated sell of the same token by the same wallet
/// (order-preserving FIFO-ish pairing). Only wallets with >= 10 completed
/// round trips are kept (matches the t-stat gate's min_samples), and each
/// wallet contributes at most its 50 most recent round trips.
const REFERENCE_SQL: &str = r#"WITH buys AS (
    SELECT
        trader_id AS wallet_address,
        token_bought_mint_address AS token_address,
        block_time AS entry_ts,
        amount_usd AS buy_usd,
        ROW_NUMBER() OVER (
            PARTITION BY trader_id, token_bought_mint_address
            ORDER BY block_time
        ) AS rn
    FROM {trades_table}
    WHERE block_time > NOW() - INTERVAL '30' day
      AND token_sold_symbol IN ('SOL', 'WSOL', 'USDC', 'USDT')
      AND trader_id IS NOT NULL
      AND token_bought_mint_address IS NOT NULL
      AND amount_usd > 50
),
sells AS (
    SELECT
        trader_id AS wallet_address,
        token_sold_mint_address AS token_address,
        block_time AS exit_ts,
        amount_usd AS sell_usd,
        ROW_NUMBER() OVER (
            PARTITION BY trader_id, token_sold_mint_address
            ORDER BY block_time
        ) AS rn
    FROM {trades_table}
    WHERE block_time > NOW() - INTERVAL '30' day
      AND token_bought_symbol IN ('SOL', 'WSOL', 'USDC', 'USDT')
      AND trader_id IS NOT NULL
      AND token_sold_mint_address IS NOT NULL
      AND amount_usd > 50
),
round_trips AS (
    SELECT
        b.wallet_address,
        b.token_address,
        (s.sell_usd - b.buy_usd) / b.buy_usd * 100.0 AS pnl_pct,
        s.exit_ts,
        date_diff('second', b.entry_ts, s.exit_ts) AS hold_duration_secs,
        ROW_NUMBER() OVER (
            PARTITION BY b.wallet_address ORDER BY s.exit_ts DESC
        ) AS rn
    FROM buys b
    JOIN sells s
      ON b.wallet_address = s.wallet_address
     AND b.token_address  = s.token_address
     AND b.rn             = s.rn
    WHERE s.exit_ts > b.entry_ts
      AND s.sell_usd > 0
      AND b.buy_usd > 0
),
wallet_counts AS (
    SELECT wallet_address, COUNT(*) AS n
    FROM round_trips
    GROUP BY wallet_address
)
SELECT
    rt.wallet_address,
    rt.token_address,
    ROUND(rt.pnl_pct, 4) AS pnl_pct,
    rt.exit_ts,
    rt.hold_duration_secs
FROM round_trips rt
JOIN wallet_counts wc ON wc.wallet_address = rt.wallet_address
WHERE wc.n >= 10
  AND rt.rn <= 50
ORDER BY rt.wallet_address, rt.exit_ts
LIMIT 20000"#;

/// Extract the first `FROM <schema>.<table>` (or `FROM <table>`) from a Dune
/// query so the bootstrap query reuses the workspace's actual dataset.
fn extract_trades_table(query_sql: &str) -> Result<String> {
    // Tokenize: find the first `FROM` keyword, then read the following
    // identifier (optionally `schema.table`, quoted or backticked).
    let lower = query_sql.to_ascii_lowercase();
    let mut idx = 0usize;
    while let Some(rel) = lower[idx..].find("from") {
        let start = idx + rel;
        // Word boundary: char before must not be alphanumeric/underscore.
        let prev = start.checked_sub(1).map(|i| lower.as_bytes()[i]);
        let prev_ok =
            prev.is_none() || !(prev.unwrap().is_ascii_alphanumeric() || prev.unwrap() == b'_');
        let next = lower.as_bytes().get(start + 4).copied();
        let next_ok =
            next.is_none() || !(next.unwrap().is_ascii_alphanumeric() || next.unwrap() == b'_');
        if prev_ok && next_ok {
            // Skip whitespace, then read the table reference.
            let rest = &query_sql[start + 4..];
            let mut chars = rest.char_indices().peekable();
            while let Some((_, c)) = chars.peek() {
                if c.is_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }
            let mut table = String::new();
            for (_, c) in chars {
                if c.is_whitespace() || c == ',' || c == ')' || c == '(' || c == ';' {
                    break;
                }
                table.push(c);
            }
            if table.is_empty() {
                bail!("FROM keyword found but no table follows in the reference query");
            }
            return Ok(table);
        }
        idx = start + 4;
    }
    bail!(
        "could not find a FROM <schema>.<table> in the reference query SQL \
         (is it a query against a dataset table?)"
    )
}

/// Build the bootstrap query SQL for the given trades table.
fn build_bootstrap_sql(trades_table: &str) -> String {
    REFERENCE_SQL.replace("{trades_table}", trades_table)
}

/// Parse one JSON result row into a BootstrapTrade. Column names are matched
/// defensively (snake_case aliases); a missing/unparseable required column is
/// an error (fail-closed).
fn parse_trade(row: &serde_json::Value) -> Result<BootstrapTrade> {
    let wallet_address = get_str(row, &["wallet_address", "wallet", "address"])?
        .ok_or_else(|| anyhow!("missing wallet_address column"))?;
    if wallet_address.len() < 32 {
        bail!("wallet_address too short: {wallet_address}");
    }
    let token_address = get_str(row, &["token_address", "token", "mint"])?
        .ok_or_else(|| anyhow!("missing token_address column"))?;
    let pnl_pct =
        get_decimal(row, &["pnl_pct", "pnl"])?.ok_or_else(|| anyhow!("missing pnl_pct column"))?;
    let exit_ts = get_ts(row, &["exit_ts", "exit_time", "closed_at", "exited_at"])?
        .ok_or_else(|| anyhow!("missing exit_ts column"))?;
    let hold_duration_secs = get_i64(row, &["hold_duration_secs", "hold_secs"])
        .ok()
        .flatten();
    Ok(BootstrapTrade {
        wallet_address,
        token_address,
        pnl_pct,
        exit_ts,
        hold_duration_secs,
    })
}

fn get_str(row: &serde_json::Value, keys: &[&str]) -> Result<Option<String>> {
    for k in keys {
        if let Some(v) = row.get(*k) {
            return match v {
                serde_json::Value::String(s) => Ok(Some(s.clone())),
                serde_json::Value::Null => Ok(None),
                other => Ok(Some(other.to_string())),
            };
        }
    }
    Ok(None)
}

/// Like `get_f64` but preserves full decimal precision (financial values).
fn get_decimal(row: &serde_json::Value, keys: &[&str]) -> Result<Option<Decimal>> {
    for k in keys {
        if let Some(v) = row.get(*k) {
            return match v {
                serde_json::Value::Number(n) => {
                    if let Some(s) = n.as_i64() {
                        Ok(Some(Decimal::from(s)))
                    } else {
                        Decimal::from_str(&n.to_string())
                            .map(Some)
                            .map_err(|e| anyhow!("column {k} is not a valid decimal: {e}"))
                    }
                }
                serde_json::Value::String(s) => {
                    if s.trim().is_empty() {
                        Ok(None)
                    } else {
                        Decimal::from_str(s.trim())
                            .map(Some)
                            .map_err(|e| anyhow!("column {k} '{s}' is not a decimal: {e}"))
                    }
                }
                serde_json::Value::Null => Ok(None),
                other => bail!("column {k} has unexpected type: {other}"),
            };
        }
    }
    Ok(None)
}

fn get_i64(row: &serde_json::Value, keys: &[&str]) -> Result<Option<i64>> {
    for k in keys {
        if let Some(v) = row.get(*k) {
            return match v {
                serde_json::Value::Number(n) => n
                    .as_i64()
                    .map(Some)
                    .ok_or_else(|| anyhow!("column {k} is not an integer")),
                serde_json::Value::String(s) => {
                    if s.trim().is_empty() {
                        Ok(None)
                    } else {
                        s.trim()
                            .parse::<i64>()
                            .map(Some)
                            .map_err(|e| anyhow!("column {k} '{s}' is not an integer: {e}"))
                    }
                }
                serde_json::Value::Null => Ok(None),
                other => bail!("column {k} has unexpected type: {other}"),
            };
        }
    }
    Ok(None)
}

fn get_ts(row: &serde_json::Value, keys: &[&str]) -> Result<Option<DateTime<Utc>>> {
    for k in keys {
        if let Some(v) = row.get(*k) {
            return match v {
                serde_json::Value::Number(n) => {
                    let secs = n
                        .as_f64()
                        .ok_or_else(|| anyhow!("column {k} not a number"))?;
                    // Dune may return epoch seconds or milliseconds.
                    let secs = if secs > 1e11 { secs / 1000.0 } else { secs };
                    DateTime::from_timestamp(secs as i64, 0)
                        .map(Some)
                        .ok_or_else(|| anyhow!("column {k} timestamp out of range"))
                }
                serde_json::Value::String(s) => {
                    if s.trim().is_empty() {
                        return Ok(None);
                    }
                    let s = s.trim();
                    let parsed = DateTime::parse_from_rfc3339(s)
                        .map(|dt| dt.with_timezone(&Utc))
                        .or_else(|_| {
                            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
                                .map(|nd| nd.and_utc())
                        })
                        .or_else(|_| {
                            // Dune renders timestamps as "...000 UTC".
                            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC")
                                .map(|nd| nd.and_utc())
                        })
                        .or_else(|_| {
                            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S UTC")
                                .map(|nd| nd.and_utc())
                        })
                        .or_else(|_| {
                            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                                .map(|nd| nd.and_utc())
                        })
                        .or_else(|_| {
                            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                                .map(|nd| nd.and_utc())
                        });
                    match parsed {
                        Ok(dt) => Ok(Some(dt)),
                        Err(_) => bail!("column {k} timestamp '{s}' is not parseable"),
                    }
                }
                serde_json::Value::Null => Ok(None),
                other => bail!("column {k} has unexpected type: {other}"),
            };
        }
    }
    Ok(None)
}

/// Replace a wallet's `dune_%` bootstrap rows with fresh ones, in one
/// transaction. Idempotent: re-runs delete first (the
/// `UNIQUE(shadow_id, exit_strategy)` constraint would otherwise collide).
async fn replace_wallet_trades(
    db: &Arc<dyn Database>,
    wallet: &str,
    trades: &[BootstrapTrade],
) -> Result<usize> {
    let DbPool::PostgreSQL(pool) = db.pool();
    let mut tx = pool.begin().await.context("failed to begin transaction")?;

    // Delete existing bootstrap evidence for this wallet. shadow_exits rows
    // are removed via the FK ON DELETE CASCADE, but deleting them explicitly
    // by shadow_id keeps the intent visible and is order-independent.
    sqlx::query(
        "DELETE FROM shadow_exits \
         WHERE shadow_id IN (SELECT shadow_id FROM shadow_positions \
                             WHERE wallet_address = $1 AND shadow_id LIKE 'dune%')",
    )
    .bind(wallet)
    .execute(&mut *tx)
    .await
    .context("failed to delete old dune shadow_exits")?;
    sqlx::query(
        "DELETE FROM shadow_positions \
         WHERE wallet_address = $1 AND shadow_id LIKE 'dune%'",
    )
    .bind(wallet)
    .execute(&mut *tx)
    .await
    .context("failed to delete old dune shadow_positions")?;

    let entry_amount_sol = Decimal::from_str(ENTRY_AMOUNT_SOL).unwrap();
    for (idx, t) in trades.iter().enumerate() {
        // wallet addresses are ASCII base58, so a byte prefix is safe; the
        // index keeps shadow_id UNIQUE across re-runs within a wallet.
        let prefix = &wallet[..wallet.len().min(8)];
        let shadow_id = format!("dune_{prefix}_{idx}");
        sqlx::query(
            "INSERT INTO shadow_positions \
                (shadow_id, wallet_address, token_address, main_admitted, \
                 entry_amount_sol, ingress, opened_at, fully_closed) \
             VALUES ($1, $2, $3, false, $4, 'Dune', $5, true)",
        )
        .bind(&shadow_id)
        .bind(wallet)
        .bind(&t.token_address)
        .bind(entry_amount_sol)
        .bind(t.exit_ts)
        .execute(&mut *tx)
        .await
        .context("failed to insert dune shadow_position")?;

        let pnl_sol = (t.pnl_pct / Decimal::from(100)) * entry_amount_sol;
        let pnl_sol = pnl_sol.round_dp(18);
        sqlx::query(
            "INSERT INTO shadow_exits \
                (shadow_id, exit_strategy, pnl_pct, pnl_sol, exit_reason, \
                 hold_duration_secs, exited_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&shadow_id)
        .bind(EXIT_STRATEGY)
        .bind(t.pnl_pct)
        .bind(pnl_sol)
        .bind(EXIT_REASON)
        .bind(t.hold_duration_secs)
        .bind(t.exit_ts)
        .execute(&mut *tx)
        .await
        .context("failed to insert dune shadow_exit")?;
    }

    tx.commit().await.context("failed to commit transaction")?;
    Ok(trades.len())
}

/// Seed the wallets roster: INSERT missing wallets as CANDIDATE
/// (ON CONFLICT DO NOTHING), update win_rate for existing CANDIDATE rows.
/// Never touches ACTIVE/REJECTED status (the UPDATE is guarded by
/// status='CANDIDATE').
async fn seed_roster(
    db: &Arc<dyn Database>,
    summaries: &[WalletSummary],
) -> Result<(usize, usize)> {
    let DbPool::PostgreSQL(pool) = db.pool();
    let mut inserted = 0usize;
    let mut updated = 0usize;
    for s in summaries {
        let wqs = s.wqs_heuristic();
        let win_rate = s.win_rate();
        let res = sqlx::query(
            "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, win_rate) \
             VALUES ($1, 'CANDIDATE', $2, 0.5, $3) \
             ON CONFLICT (address) DO NOTHING",
        )
        .bind(&s.address)
        .bind(wqs)
        .bind(win_rate)
        .execute(&pool)
        .await
        .context("failed to insert candidate wallet")?;
        if res.rows_affected() > 0 {
            inserted += 1;
        } else {
            // Existing row: refresh win_rate for CANDIDATE only. The status
            // guard means ACTIVE/REJECTED rows are never modified.
            let upd = sqlx::query(
                "UPDATE wallets SET win_rate = $2, updated_at = NOW() \
                 WHERE address = $1 AND status = 'CANDIDATE'",
            )
            .bind(&s.address)
            .bind(win_rate)
            .execute(&pool)
            .await
            .context("failed to update candidate win_rate")?;
            if upd.rows_affected() > 0 {
                updated += 1;
            }
        }
    }
    Ok((inserted, updated))
}

async fn run() -> Result<()> {
    init_tracing();
    let args = parse_args()?;
    let config = load_config()?;

    if !config.dune.enabled {
        bail!("dune.enabled is false in config — bootstrap refuses to run");
    }
    let api_key = std::env::var("DUNE_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        bail!("DUNE_API_KEY is not set");
    }
    let client = DuneClient::new(api_key)?;

    // 0. Query ID: use config; create via the Dune API when requested and
    //    no query is configured yet. `--from-query` controls the reference
    //    query whose trades table is reused (default: dune.promote_query_id).
    let mut query_id = config.dune.bootstrap_query_id;
    if args.create_query && query_id == 0 {
        let from_query = args.from_query.unwrap_or(config.dune.promote_query_id);
        let reference_sql = client.fetch_query_sql(from_query).await?;
        let table = extract_trades_table(&reference_sql)?;
        let sql = build_bootstrap_sql(&table);
        query_id = client
            .create_query("Chimera Bootstrap — Wallet Round-Trip PnL (30d)", &sql)
            .await?;
        info!(
            query_id,
            table = %table,
            from_query,
            "bootstrap: created Dune query via API — set dune.bootstrap_query_id={query_id} \
             in config/config.yaml so server runs reuse it"
        );
    }
    if query_id == 0 {
        bail!(
            "dune.bootstrap_query_id is 0 and --create-query was not used — set the \
             bootstrap query ID in config/config.yaml or pass --create-query"
        );
    }

    info!(
        query_id,
        dry_run = args.dry_run,
        roster_enabled = config.dune.bootstrap_roster_enabled,
        wallets_max = config.dune.bootstrap_wallets_max,
        "Dune bootstrap started"
    );

    // 1. Fetch + parse (fail-closed on Dune API / parse errors).
    let trades = client.fetch_bootstrap_trades(query_id).await?;
    if trades.is_empty() {
        bail!("Dune query returned 0 rows — aborting (fail-closed, no writes)");
    }

    // 2. Group by wallet, order by row count desc (top traders first), cap.
    let mut by_wallet: BTreeMap<String, Vec<BootstrapTrade>> = BTreeMap::new();
    for t in trades {
        by_wallet
            .entry(t.wallet_address.clone())
            .or_default()
            .push(t);
    }
    if let Some(ref target) = args.wallet {
        by_wallet.retain(|addr, _| addr == target);
        if by_wallet.is_empty() {
            bail!("--wallet {target} not present in the Dune result");
        }
    }
    let mut summaries: Vec<WalletSummary> = by_wallet
        .into_iter()
        .map(|(address, mut ts)| {
            ts.sort_by_key(|t| t.exit_ts);
            WalletSummary {
                address,
                trades: ts,
            }
        })
        .collect();
    summaries.sort_by(|a, b| {
        b.trades
            .len()
            .cmp(&a.trades.len())
            .then_with(|| a.address.cmp(&b.address))
    });
    let max_wallets = config.dune.bootstrap_wallets_max.max(1);
    let total_found = summaries.len();
    summaries.truncate(max_wallets);

    // 3. Per-wallet report.
    let mut total_rows = 0usize;
    let mut profitable = 0usize;
    for s in &summaries {
        let t = s.t_stat();
        let verdict = if s.passes_tstat() {
            profitable += 1;
            "PROFITABLE"
        } else {
            "INSUFFICIENT"
        };
        info!(
            wallet = %s.address,
            n = s.trades.len(),
            mean_pnl_pct = %s.mean_pnl_pct().round_dp(2),
            stddev_pnl_pct = %s.stddev_pnl_pct().round_dp(2),
            t_stat = format_args!("{t:.3}"),
            threshold = TSTAT_THRESHOLD,
            win_rate = format_args!("{:.3}", s.win_rate()),
            wqs_heuristic = format_args!("{:.1}", s.wqs_heuristic()),
            verdict,
            "bootstrap wallet"
        );
        total_rows += s.trades.len();
    }
    info!(
        wallets_found = total_found,
        wallets_processed = summaries.len(),
        wallets_profitable = profitable,
        total_rows,
        "bootstrap dry-run summary"
    );

    if args.dry_run {
        info!(
            "DRY RUN — no writes performed. Re-run with --apply to write, \
             --apply --roster to also seed the wallets roster"
        );
        return Ok(());
    }

    // DB pool is only needed for writes (dry-run reports need no database).
    let db_config = DatabaseConfig {
        backend: DatabaseBackend::from_env(),
        url: config.database.url.clone(),
        max_connections: config.database.max_connections,
        acquire_timeout_seconds: 30,
    };
    let db = create_database(&db_config)
        .await
        .context("database init failed")?;

    // 4. Write shadow rows per wallet.
    let mut written = 0usize;
    for s in &summaries {
        match replace_wallet_trades(&db, &s.address, &s.trades).await {
            Ok(n) => {
                written += n;
                info!(wallet = %s.address, rows = n, "bootstrap: replaced dune_% rows");
            }
            Err(e) => {
                error!(wallet = %s.address, error = %e, "bootstrap: wallet insert failed");
                bail!("bootstrap aborted after partial write — re-run is idempotent: {e}");
            }
        }
    }
    info!(
        written,
        "bootstrap: shadow rows written (exit_strategy=dune_wallet)"
    );

    // 5. Roster seed (CANDIDATE only).
    let roster_enabled = (config.dune.bootstrap_roster_enabled || args.roster) && !args.no_roster;
    if roster_enabled {
        let (inserted, updated) = seed_roster(&db, &summaries).await?;
        info!(
            inserted,
            updated, "bootstrap: roster seeded (CANDIDATE only; ACTIVE/REJECTED untouched)"
        );
    } else {
        info!("bootstrap: roster seeding skipped (--roster not requested or disabled)");
    }

    info!("Dune bootstrap completed");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("bootstrap_dune failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}
