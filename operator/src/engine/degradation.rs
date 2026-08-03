//! Graceful Degradation Handlers
//!
//! Implements automatic recovery and degradation strategies for various failure modes:
//! - Memory pressure monitoring and load shedding
//! - Disk space monitoring and log pruning
//! - RPC rate limit handling with exponential backoff
//!
//! Note: SQLite lock-retry helpers were removed when SQLite was decommissioned
//! (2026-07); PostgreSQL uses its own transaction/serialization handling.

use crate::error::{AppError, AppResult};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Initial backoff delay in milliseconds
const INITIAL_BACKOFF_MS: u64 = 100;

/// Maximum backoff delay in milliseconds
const MAX_BACKOFF_MS: u64 = 5000;

/// Memory pressure threshold (percentage)
const MEMORY_PRESSURE_THRESHOLD: f64 = 0.90;

/// Disk space warning threshold (percentage free)
const DISK_SPACE_WARNING_THRESHOLD: f64 = 0.10;

/// Global memory pressure flag
static MEMORY_PRESSURE: AtomicBool = AtomicBool::new(false);

/// Global RPC rate limit backoff multiplier
static RPC_BACKOFF_MULTIPLIER: AtomicU64 = AtomicU64::new(1);

/// Check memory pressure and return the current usage **fraction** (0.0–1.0).
pub async fn check_memory_pressure() -> AppResult<f64> {
    tokio::task::spawn_blocking(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();

        let total = sys.total_memory();
        let available = sys.available_memory();

        if total == 0 {
            return Err(AppError::Internal(
                "Could not determine total memory".to_string(),
            ));
        }

        let used = total.saturating_sub(available);
        let usage_percent = (used as f64 / total as f64) * 100.0;

        // Update global flag
        MEMORY_PRESSURE.store(
            usage_percent >= (MEMORY_PRESSURE_THRESHOLD * 100.0),
            Ordering::Relaxed,
        );

        Ok(usage_percent / 100.0)
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking join error: {}", e)))?
}

/// Check if memory pressure is high
pub fn is_memory_pressure_high() -> bool {
    MEMORY_PRESSURE.load(Ordering::Relaxed)
}

/// Check disk space and return free space percentage (0.0–1.0).
pub async fn check_disk_space(path: &std::path::Path) -> AppResult<f64> {
    #[cfg(unix)]
    {
        let path_str = path.to_string_lossy().to_string();
        tokio::task::spawn_blocking(move || df_free_fraction(&path_str))
            .await
            .map_err(|e| AppError::Internal(format!("spawn_blocking join error: {}", e)))?
    }

    #[cfg(not(unix))]
    {
        tracing::warn!("Disk space check not implemented for this platform, assuming 50% free");
        Ok(0.5)
    }
}

/// Run `df -k` and parse the free-space fraction (0.0–1.0).
///
/// Fail-deadly: a failed `df` run, a non-zero exit status, or malformed output
/// propagates as an `AppError` instead of being read as "0% free" (which would
/// drive `prune_logs_if_needed` into aggressive deletion on a transient error).
#[cfg(unix)]
fn df_free_fraction(path: &str) -> AppResult<f64> {
    let output = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .map_err(|e| AppError::Internal(format!("df command failed: {}", e)))?;

    if !output.status.success() {
        return Err(AppError::Internal(format!(
            "df exited with status {}",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // df -k output: header line + data line
    // Columns: Filesystem  1K-blocks  Used  Available  Use%  Mountpoint
    let line = stdout
        .lines()
        .nth(1)
        .ok_or_else(|| AppError::Internal("df output missing data line".to_string()))?;

    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 5 {
        return Err(AppError::Internal(format!(
            "Unexpected df output: {}",
            line
        )));
    }

    let total: f64 = cols[1]
        .parse()
        .map_err(|_| AppError::Internal(format!("Unparseable df total: {}", cols[1])))?;
    let avail: f64 = cols[3]
        .parse()
        .map_err(|_| AppError::Internal(format!("Unparseable df available: {}", cols[3])))?;

    if total == 0.0 {
        return Ok(0.0);
    }

    tracing::debug!(
        path = path,
        total_kb = total,
        avail_kb = avail,
        free_pct = avail / total,
        "Disk space check"
    );
    Ok(avail / total)
}

/// Handle RPC rate limit with exponential backoff
///
/// Returns the delay to wait before retrying
pub async fn handle_rpc_rate_limit() -> Duration {
    // fetch_update makes the read-and-double one atomic operation, so two
    // concurrent rate-limit hits cannot both read the same multiplier and
    // both store the same doubled value (losing an increment).
    let multiplier = RPC_BACKOFF_MULTIPLIER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |m| Some((m * 2).min(64)))
        .unwrap_or(1);
    let delay_ms = INITIAL_BACKOFF_MS * multiplier;

    // Cap at max backoff
    let capped_delay = delay_ms.min(MAX_BACKOFF_MS);

    tracing::warn!(
        multiplier = multiplier,
        delay_ms = capped_delay,
        "RPC rate limit hit, applying exponential backoff"
    );

    Duration::from_millis(capped_delay)
}

/// Reset RPC backoff (call after successful request)
pub fn reset_rpc_backoff() {
    RPC_BACKOFF_MULTIPLIER.store(1, Ordering::Relaxed);
}

/// Get current RPC backoff multiplier (for monitoring)
pub fn get_rpc_backoff_multiplier() -> u64 {
    RPC_BACKOFF_MULTIPLIER.load(Ordering::Relaxed)
}

/// Disk space critical threshold (percentage free). Below this, pruning becomes aggressive.
const DISK_SPACE_CRITICAL_THRESHOLD: f64 = 0.05;

/// Check whether a path points at one of OUR log files (active, rotated, or
/// compressed). Restricted to the operator log naming (`operator.log`,
/// `operator.log.*`, and their `.gz` forms) so unrelated `.log`/`.gz` archives
/// are never touched. The active `operator.log` itself is matched here but
/// excluded from pruning by the caller.
fn is_log_file(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.starts_with("operator.log")
                && (n == "operator.log" || n.starts_with("operator.log.") || n.ends_with(".gz"))
        })
        .unwrap_or(false)
}

/// The active (currently open) log file — never pruned: writes after removal
/// would go to an unlinked inode and be lost permanently.
fn is_active_log_file(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == "operator.log")
        .unwrap_or(false)
}

/// Synchronous free-space fraction, used between deletions inside the pruning
/// loop (which runs in `spawn_blocking`).
#[cfg(unix)]
fn check_disk_space_sync(path: &std::path::Path) -> AppResult<f64> {
    df_free_fraction(&path.to_string_lossy())
}

#[cfg(not(unix))]
fn check_disk_space_sync(_path: &std::path::Path) -> AppResult<f64> {
    Ok(0.5)
}

/// Prune old log files if disk space is below the warning threshold.
/// Deletes `.log`, rotated (`operator.log.*`) and compressed (`.gz`) files
/// in `log_dir` older than `max_age_days`. Below the critical threshold,
/// shortens max_age to 1 day and includes all rotated/compressed logs.
/// The active `operator.log` is never pruned.
pub async fn prune_logs_if_needed(log_dir: &std::path::Path, max_age_days: u32) -> AppResult<()> {
    let free_space = check_disk_space(log_dir).await?;

    if free_space >= DISK_SPACE_WARNING_THRESHOLD {
        return Ok(());
    }

    let critical = free_space < DISK_SPACE_CRITICAL_THRESHOLD;
    let effective_max_age_days = if critical { 1u32 } else { max_age_days };
    let effective_max_age = std::time::Duration::from_secs(effective_max_age_days as u64 * 86400);

    tracing::warn!(
        free_space_pct = free_space * 100.0,
        threshold_pct = DISK_SPACE_WARNING_THRESHOLD * 100.0,
        critical = critical,
        max_age_days = effective_max_age_days,
        "Disk space low, pruning old log files"
    );

    let log_dir_owned = log_dir.to_path_buf();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let now = std::time::SystemTime::now();
        let mut pruned = 0u32;
        let mut bytes_freed: u64 = 0;

        let entries = std::fs::read_dir(&log_dir_owned)
            .map_err(|e| AppError::Internal(format!("Failed to read log dir: {}", e)))?;

        let mut candidates: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_log_file(&path) || is_active_log_file(&path) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    candidates.push((modified, meta.len(), path));
                }
            }
        }

        // Phase 1: age-based pruning
        for (modified, size, ref path) in &candidates {
            if let Ok(age) = now.duration_since(*modified) {
                if age > effective_max_age && std::fs::remove_file(path).is_ok() {
                    pruned += 1;
                    bytes_freed += size;
                    tracing::debug!(file = ?path, age_days = age.as_secs() / 86400, "Pruned log file");
                }
            }
        }

        // Phase 2: size-based capping — delete oldest remaining files until
        // threshold met. Abort on a failed re-check (never read a parse failure
        // as "still critically full"), and re-check at most every 5 deletions
        // to avoid spawning a `df` subprocess per file.
        let mut remaining_free = check_disk_space_sync(&log_dir_owned)?;
        if remaining_free < DISK_SPACE_WARNING_THRESHOLD {
            let mut remaining: Vec<_> = candidates
                .iter()
                .filter(|(_, _, path)| std::fs::metadata(path).is_ok())
                .collect();
            remaining.sort_by_key(|a| a.0);

            let mut deletions_since_check = 0u32;
            for (_, size, path) in remaining {
                if std::fs::remove_file(path).is_ok() {
                    pruned += 1;
                    bytes_freed += size;
                    tracing::debug!(file = ?path, "Pruned log file (size-based cap)");
                    deletions_since_check += 1;
                    if deletions_since_check >= 5 {
                        remaining_free = check_disk_space_sync(&log_dir_owned)?;
                        if remaining_free >= DISK_SPACE_WARNING_THRESHOLD {
                            break;
                        }
                        deletions_since_check = 0;
                    }
                }
            }
        }

        tracing::info!(pruned_files = pruned, bytes_freed = bytes_freed, "Log pruning complete");
        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking join error: {}", e)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_memory_pressure_flag() {
        MEMORY_PRESSURE.store(false, Ordering::Relaxed);
        assert!(!is_memory_pressure_high());

        MEMORY_PRESSURE.store(true, Ordering::Relaxed);
        assert!(is_memory_pressure_high());
    }

    #[test]
    fn test_rpc_backoff_reset() {
        RPC_BACKOFF_MULTIPLIER.store(8, Ordering::Relaxed);
        reset_rpc_backoff();
        assert_eq!(RPC_BACKOFF_MULTIPLIER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_is_log_file() {
        let cases = [
            (PathBuf::from("operator.log"), true),
            (PathBuf::from("operator.log.1"), true),
            (PathBuf::from("operator.log.2.gz"), true),
            (PathBuf::from("something.gz"), false),
            (PathBuf::from("other.log"), false),
            (PathBuf::from("config.yaml"), false),
            (PathBuf::from("backup.db"), false),
            (PathBuf::from("README.md"), false),
        ];
        for (path, expected) in cases {
            assert_eq!(is_log_file(&path), expected, "path: {:?}", path);
        }
    }
}
