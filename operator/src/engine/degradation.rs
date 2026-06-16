//! Graceful Degradation Handlers
//!
//! Implements automatic recovery and degradation strategies for various failure modes:
//! - SQLite lock retry with backoff
//! - Memory pressure monitoring and load shedding
//! - Disk space monitoring and log pruning
//! - RPC rate limit handling with exponential backoff

use crate::error::{AppError, AppResult};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::sleep;

/// Maximum retry attempts for SQLite operations
const MAX_SQLITE_RETRIES: u32 = 3;

/// Initial backoff delay in milliseconds
const INITIAL_BACKOFF_MS: u64 = 100;

/// Maximum backoff delay in milliseconds
const MAX_BACKOFF_MS: u64 = 5000;

/// Memory pressure threshold (percentage)
#[allow(dead_code)] // Reserved for future memory pressure monitoring
const MEMORY_PRESSURE_THRESHOLD: f64 = 0.90;

/// Disk space warning threshold (percentage free)
const DISK_SPACE_WARNING_THRESHOLD: f64 = 0.10;

/// Global memory pressure flag
static MEMORY_PRESSURE: AtomicBool = AtomicBool::new(false);

/// Global RPC rate limit backoff multiplier
static RPC_BACKOFF_MULTIPLIER: AtomicU64 = AtomicU64::new(1);

/// Retry SQLite operation with exponential backoff
///
/// This handles SQLite lock errors by retrying with increasing delays.
pub async fn retry_sqlite<F, T, E>(operation: F) -> Result<T, E>
where
    F: Fn() -> Result<T, E> + Send + Sync,
    E: std::fmt::Display + Send + Sync,
{
    let mut attempt = 0;
    let mut backoff_ms = INITIAL_BACKOFF_MS;

    loop {
        match operation() {
            Ok(result) => {
                // Reset backoff on success
                RPC_BACKOFF_MULTIPLIER.store(1, Ordering::Relaxed);
                return Ok(result);
            }
            Err(e) => {
                let error_str = e.to_string().to_lowercase();

                // Check if it's a lock error
                if error_str.contains("locked")
                    || error_str.contains("database is locked")
                    || error_str.contains("busy")
                {
                    attempt += 1;

                    if attempt >= MAX_SQLITE_RETRIES {
                        tracing::error!(
                            attempt = attempt,
                            error = %e,
                            "SQLite operation failed after max retries"
                        );
                        return Err(e);
                    }

                    tracing::warn!(
                        attempt = attempt,
                        backoff_ms = backoff_ms,
                        error = %e,
                        "SQLite lock detected, retrying with backoff"
                    );

                    sleep(Duration::from_millis(backoff_ms)).await;

                    // Exponential backoff: double the delay, cap at max
                    backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                } else {
                    // Not a lock error, return immediately
                    return Err(e);
                }
            }
        }
    }
}

/// Check memory pressure and return current usage percentage
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

/// Check disk space and return free space percentage (0.0–1.0)
pub async fn check_disk_space(path: &std::path::Path) -> AppResult<f64> {
    #[cfg(unix)]
    {
        let path_str = path.to_string_lossy().to_string();
        tokio::task::spawn_blocking(move || -> AppResult<f64> {
            let output = std::process::Command::new("df")
                .arg("-k")
                .arg(&path_str)
                .output()
                .map_err(|e| AppError::Internal(format!("df command failed: {}", e)))?;

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

            let total: f64 = cols[1].parse().unwrap_or(1.0);
            let avail: f64 = cols[3].parse().unwrap_or(0.0);

            if total == 0.0 {
                return Ok(0.0);
            }

            tracing::debug!(
                path = path_str,
                total_kb = total,
                avail_kb = avail,
                free_pct = avail / total,
                "Disk space check"
            );
            Ok(avail / total)
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking join error: {}", e)))?
    }

    #[cfg(not(unix))]
    {
        tracing::warn!("Disk space check not implemented for this platform, assuming 50% free");
        Ok(0.5)
    }
}

/// Handle RPC rate limit with exponential backoff
///
/// Returns the delay to wait before retrying
pub async fn handle_rpc_rate_limit() -> Duration {
    let multiplier = RPC_BACKOFF_MULTIPLIER.load(Ordering::Relaxed);
    let delay_ms = INITIAL_BACKOFF_MS * multiplier;

    // Cap at max backoff
    let capped_delay = delay_ms.min(MAX_BACKOFF_MS);

    tracing::warn!(
        multiplier = multiplier,
        delay_ms = capped_delay,
        "RPC rate limit hit, applying exponential backoff"
    );

    // Increase multiplier for next time (with cap)
    let new_multiplier = (multiplier * 2).min(64);
    RPC_BACKOFF_MULTIPLIER.store(new_multiplier, Ordering::Relaxed);

    Duration::from_millis(capped_delay)
}

/// Reset RPC backoff (call after successful request)
pub fn reset_rpc_backoff() {
    RPC_BACKOFF_MULTIPLIER.store(1, Ordering::Relaxed);
}

/// Prune old log files if disk space is below the warning threshold.
/// Deletes `.log` files in `log_dir` that are older than `max_age_days`.
pub async fn prune_logs_if_needed(log_dir: &std::path::Path, max_age_days: u32) -> AppResult<()> {
    let free_space = check_disk_space(log_dir).await?;

    if free_space >= DISK_SPACE_WARNING_THRESHOLD {
        return Ok(());
    }

    tracing::warn!(
        free_space_pct = free_space * 100.0,
        threshold_pct = DISK_SPACE_WARNING_THRESHOLD * 100.0,
        max_age_days = max_age_days,
        "Disk space low, pruning old log files"
    );

    let log_dir_owned = log_dir.to_path_buf();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let max_age = std::time::Duration::from_secs(max_age_days as u64 * 86400);
        let now = std::time::SystemTime::now();
        let mut pruned = 0u32;

        let entries = std::fs::read_dir(&log_dir_owned)
            .map_err(|e| AppError::Internal(format!("Failed to read log dir: {}", e)))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age
                            && std::fs::remove_file(&path).is_ok() {
                                pruned += 1;
                                tracing::debug!(file = ?path, age_days = age.as_secs() / 86400, "Pruned log file");
                            }
                    }
                }
            }
        }

        tracing::info!(pruned_files = pruned, "Log pruning complete");
        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("spawn_blocking join error: {}", e)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_retry_sqlite_success() {
        use std::sync::{Arc, Mutex};
        let attempts = Arc::new(Mutex::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_sqlite(move || {
            let mut count = attempts_clone.lock().unwrap();
            *count += 1;
            if *count == 1 {
                Err("database is locked")
            } else {
                Ok(42)
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(*attempts.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn test_retry_sqlite_max_retries() {
        let result = retry_sqlite(|| Err::<i32, _>("database is locked")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_retry_sqlite_non_lock_error() {
        let result = retry_sqlite(|| Err::<i32, _>("other error")).await;
        assert!(result.is_err());
    }

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
}
