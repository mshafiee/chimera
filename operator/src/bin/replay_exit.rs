//! Offline exit-rule replay harness (Phase 2C).
//!
//! Replays reconstructed price paths through the SAME exit rules the live
//! monitor uses (`exit_rules::evaluate_exit`) with configurable parameter
//! overrides, so a grid-search over exit parameters can never drift from
//! production behavior.
//!
//! Usage:
//!   replay_exit --input <paths.json> [--out <results.json>]
//!
//! Input JSON shape:
//! {
//!   "overrides": { "recovery_gate_threshold": -0.02, "defer_max_ticks": 3, ... },
//!   "positions": [
//!     { "entry_price": 0.000123, "opened_at": 1787000000,
//!       "strategy": "SHIELD", "size_sol": 1.0,
//!       "points": [[1700000000, 0.000120], [1700000060, 0.000150]] }
//!   ]
//! }
//!
//! Output JSON shape:
//! { "results": [ { "entry_price":.., "exit_reason":.., "pnl_pct":..,
//!                  "pnl_sol":.., "exit_secs":.. } ] }

use std::env;
use std::fs;
use std::process;

use chimera_operator::config::ProfitManagementConfig;
use chimera_operator::engine::exit_profile::EffectiveExitParams;
use chimera_operator::engine::exit_rules::evaluate_exit;

#[derive(serde::Deserialize, serde::Serialize)]
struct Position {
    entry_price: rust_decimal::Decimal,
    #[serde(default)]
    opened_at: i64, // epoch seconds
    #[serde(default = "default_strategy")]
    strategy: String,
    #[serde(default = "default_size")]
    size_sol: rust_decimal::Decimal,
    points: Vec<(i64, rust_decimal::Decimal)>, // (ts_unix, price)
}

fn default_strategy() -> String {
    "SHIELD".to_string()
}
fn default_size() -> rust_decimal::Decimal {
    rust_decimal::Decimal::ONE
}

#[derive(serde::Deserialize, Default)]
struct Overrides {
    #[serde(default)]
    recovery_gate_threshold: Option<rust_decimal::Decimal>,
    #[serde(default)]
    recovery_gate_hard_threshold: Option<rust_decimal::Decimal>,
    #[serde(default)]
    recovery_gate_max_secs: Option<u64>,
    #[serde(default)]
    max_stop_loss_distance: Option<rust_decimal::Decimal>,
    #[serde(default)]
    wick_protection_max_loss_percent: Option<rust_decimal::Decimal>,
    #[serde(default)]
    trailing_stop_activation: Option<rust_decimal::Decimal>,
    #[serde(default)]
    trailing_stop_distance: Option<rust_decimal::Decimal>,
    #[serde(default)]
    losing_time_exit_hours_shield: Option<u64>,
    #[serde(default)]
    losing_time_exit_hours_spear: Option<u64>,
    #[serde(default)]
    time_exit_hours: Option<u64>,
}

impl Overrides {
    fn apply(&self, cfg: &mut ProfitManagementConfig) {
        if let Some(v) = self.recovery_gate_threshold {
            cfg.recovery_gate_threshold = v;
        }
        if let Some(v) = self.recovery_gate_hard_threshold {
            cfg.recovery_gate_hard_threshold = v;
        }
        if let Some(v) = self.recovery_gate_max_secs {
            cfg.recovery_gate_max_secs = v;
        }
        if let Some(v) = self.max_stop_loss_distance {
            cfg.max_stop_loss_distance = v;
        }
        if let Some(v) = self.wick_protection_max_loss_percent {
            cfg.wick_protection_max_loss_percent = v;
        }
        if let Some(v) = self.trailing_stop_activation {
            cfg.trailing_stop_activation = v;
        }
        if let Some(v) = self.trailing_stop_distance {
            cfg.trailing_stop_distance = v;
        }
        if let Some(v) = self.losing_time_exit_hours_shield {
            cfg.losing_time_exit_hours_shield = v;
        }
        if let Some(v) = self.losing_time_exit_hours_spear {
            cfg.losing_time_exit_hours_spear = v;
        }
        if let Some(v) = self.time_exit_hours {
            cfg.time_exit_hours = v;
        }
    }
}

#[derive(serde::Deserialize)]
struct Input {
    #[serde(default)]
    overrides: Overrides,
    positions: Vec<Position>,
}

#[derive(serde::Serialize)]
struct Result {
    entry_price: rust_decimal::Decimal,
    exit_reason: String,
    pnl_pct: rust_decimal::Decimal,
    pnl_sol: rust_decimal::Decimal,
    exit_secs: i64,
}

#[derive(serde::Serialize)]
struct Output {
    results: Vec<Result>,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut input = None;
    let mut out = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--input" => {
                i += 1;
                input = args.get(i).cloned();
            }
            "--out" => {
                i += 1;
                out = args.get(i).cloned();
            }
            "-h" | "--help" => {
                eprintln!("usage: replay_exit --input <paths.json> [--out results.json]");
                process::exit(2);
            }
            _ => {
                eprintln!("unknown arg: {}", args[i]);
                process::exit(2);
            }
        }
        i += 1;
    }
    let input_path = match input {
        Some(p) => p,
        None => {
            eprintln!("usage: replay_exit --input <paths.json> [--out results.json]");
            process::exit(2);
        }
    };

    let raw = if input_path == "-" {
        use std::io::{Read, stdin};
        let mut s = String::new();
        stdin()
            .lock()
            .read_to_string(&mut s)
            .unwrap_or_else(|e| {
                eprintln!("failed to read stdin: {e}");
                process::exit(1);
            });
        s
    } else {
        fs::read_to_string(&input_path).unwrap_or_else(|e| {
            eprintln!("failed to read input: {e}");
            process::exit(1);
        })
    };
    let inp: Input = serde_json::from_str(&raw).unwrap_or_else(|e| {
        eprintln!("failed to parse input: {e}");
        process::exit(1);
    });

    let mut cfg = ProfitManagementConfig::default();
    inp.overrides.apply(&mut cfg);

    let mut results = Vec::with_capacity(inp.positions.len());
    let mut n_no_points = 0usize;
    let mut n_held = 0usize;

    for pos in &inp.positions {
        if pos.points.is_empty() {
            n_no_points += 1;
            continue;
        }
        let mut eff = EffectiveExitParams::from_config(&cfg, &pos.strategy);
        // Only the trailing params feed evaluate_exit; keep per-wallet hours
        // from profile defaults (no per-wallet profile in the replay input yet).
        eff.trailing_activation_pct = cfg.trailing_stop_activation;
        eff.trailing_distance_pct = cfg.trailing_stop_distance;

        let mut peak = pos.entry_price;
        let mut result = None;
        for (ts, price) in &pos.points {
            if *price > peak {
                peak = *price;
            }
            let elapsed = if pos.opened_at > 0 {
                (ts - pos.opened_at).max(0)
            } else {
                0
            };
            if let Some(reason) = evaluate_exit(
                &cfg,
                &eff,
                pos.entry_price,
                *price,
                peak,
                elapsed,
                &pos.strategy,
            ) {
                let pnl_pct = pnl_pct_for(pos.entry_price, *price);
                let pnl_sol = pos.size_sol * pnl_pct / rust_decimal::Decimal::from(100);
                results.push(Result {
                    entry_price: pos.entry_price,
                    exit_reason: reason,
                    pnl_pct,
                    pnl_sol,
                    exit_secs: elapsed,
                });
                result = Some(());
                break;
            }
        }
        if result.is_none() {
            n_held += 1;
            // Held to end of window: exit at last point.
            let (last_ts, last_price) = *pos.points.last().unwrap();
            let elapsed = last_ts - pos.opened_at;
            let pnl_pct = pnl_pct_for(pos.entry_price, last_price);
            let pnl_sol = pos.size_sol * pnl_pct / rust_decimal::Decimal::from(100);
            results.push(Result {
                entry_price: pos.entry_price,
                exit_reason: "max_lifetime".to_string(),
                pnl_pct,
                pnl_sol,
                exit_secs: elapsed,
            });
        }
    }

    let result_count = results.len();
    let output = Output { results };
    let text = serde_json::to_string_pretty(&output).unwrap_or_else(|e| {
        eprintln!("serialize failed: {e}");
        process::exit(1);
    });
    match out {
        Some(path) => {
            fs::write(&path, text).unwrap_or_else(|e| {
                eprintln!("write failed: {e}");
                process::exit(1);
            });
        }
        None => println!("{text}"),
    }
    eprintln!(
        "replay: {} positions, {} held to end, {} with no points",
        result_count, n_held, n_no_points
    );
}

fn pnl_pct_for(entry: rust_decimal::Decimal, price: rust_decimal::Decimal) -> rust_decimal::Decimal {
    if entry.is_zero() {
        return rust_decimal::Decimal::ZERO;
    }
    (price - entry) / entry * rust_decimal::Decimal::from(100)
}
