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

    #[tokio::test]
    async fn test_check_memory_pressure_real() {
        MEMORY_PRESSURE.store(false, Ordering::Relaxed);
        let usage = check_memory_pressure().await.expect("memory check should work");
        assert!(usage > 0.0 && usage <= 1.0);
        // Flag reflects the measured usage, whatever it is
        assert_eq!(is_memory_pressure_high(), usage >= MEMORY_PRESSURE_THRESHOLD);
    }

    #[tokio::test]
    async fn test_check_disk_space_real() {
        let free = check_disk_space(std::path::Path::new("/"))
            .await
            .expect("disk check should work");
        assert!(free > 0.0 && free <= 1.0);
    }

    #[test]
    fn test_df_free_fraction_success() {
        let frac = df_free_fraction("/").expect("df on / should succeed");
        assert!(frac > 0.0 && frac <= 1.0);
    }

    #[test]
    fn test_df_free_fraction_failure() {
        // A nonexistent path makes df exit non-zero -> error, not "0% free"
        let err = df_free_fraction("/nonexistent_chimera_dir_xyz_12345").unwrap_err();
        assert!(err.to_string().contains("df exited with status"));
    }

    // The RPC backoff multiplier is a process-global static; parallel tests
    // would race on it. Serialize the tests that touch it.
    static BACKOFF_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn test_rpc_backoff_doubles_and_caps() {
        let _guard = BACKOFF_LOCK.lock().unwrap();
        reset_rpc_backoff();
        assert_eq!(get_rpc_backoff_multiplier(), 1);

        let mut delays = Vec::new();
        for _ in 0..8 {
            delays.push(handle_rpc_rate_limit().await);
        }
        // 100, 200, 400, 800, 1600, 3200, 5000 (capped), 5000 (capped)
        let expected = [
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(400),
            Duration::from_millis(800),
            Duration::from_millis(1600),
            Duration::from_millis(3200),
            Duration::from_millis(5000),
            Duration::from_millis(5000),
        ];
        assert_eq!(delays, expected);
        assert_eq!(get_rpc_backoff_multiplier(), 64);

        reset_rpc_backoff();
        assert_eq!(get_rpc_backoff_multiplier(), 1);
    }

    #[test]
    fn test_is_active_log_file() {
        assert!(is_active_log_file(&PathBuf::from("operator.log")));
        assert!(!is_active_log_file(&PathBuf::from("operator.log.1")));
        assert!(!is_active_log_file(&PathBuf::from("operator.log.2.gz")));
        assert!(!is_active_log_file(&PathBuf::from("other.log")));
        assert!(!is_active_log_file(&PathBuf::from("noext")));
    }

    #[test]
    fn test_rpc_backoff_reset() {
        let _guard = BACKOFF_LOCK.lock().unwrap();
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

    // ==========================================================================
    // RAM-disk tests: exercise the actual low-disk pruning loop (phases 1 & 2).
    // On macOS a RAM disk can be created without privileges via hdiutil/diskutil.
    // On other platforms (or when the tools are unavailable) the test skips.
    // ==========================================================================

    /// A mounted RAM disk that detaches itself on drop.
    struct RamDisk {
        device: String,
        mount: PathBuf,
    }

    impl RamDisk {
        /// Create a RAM disk. `diskutil` operations are serialized via a lock
        /// because concurrent volume erases race even with unique names.
        fn create(size_sectors: u64) -> Option<RamDisk> {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

            let _guard = LOCK.lock().unwrap();
            let attach = std::process::Command::new("hdiutil")
                .args(["attach", "-nomount", &format!("ram://{}", size_sectors)])
                .output()
                .ok()?;
            if !attach.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&attach.stdout);
            let device = stdout.split_whitespace().next()?.to_string();
            if !device.starts_with("/dev/disk") {
                return None;
            }
            let name = format!(
                "chimera_test_{}_{}",
                std::process::id(),
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let erase = std::process::Command::new("diskutil")
                .args(["erasevolume", "HFS+", &name, &device])
                .output()
                .ok()?;
            if !erase.status.success() {
                let _ = std::process::Command::new("hdiutil")
                    .args(["detach", &device])
                    .status();
                return None;
            }
            let mount = PathBuf::from(format!("/Volumes/{}", name));
            // Wait for the mount point to appear
            for _ in 0..50 {
                if mount.is_dir() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !mount.is_dir() {
                let _ = std::process::Command::new("hdiutil")
                    .args(["detach", &device])
                    .status();
                return None;
            }
            Some(RamDisk { device, mount })
        }

        /// Write a file filled with fixed bytes (so it cannot be compressed or
        /// sparse) and return its size in bytes.
        fn write_file(&self, name: &str, size_kb: u64) -> std::io::Result<()> {
            let mut f = std::fs::File::create(self.mount.join(name))?;
            use std::io::Write;
            let chunk = [0x5Au8; 4096];
            for _ in 0..size_kb / 4 {
                f.write_all(&chunk)?;
            }
            Ok(())
        }

        fn free_fraction(&self) -> f64 {
            df_free_fraction(self.mount.to_str().unwrap()).expect("df on ramdisk")
        }
    }

    impl Drop for RamDisk {
        fn drop(&mut self) {
            let _ = std::process::Command::new("hdiutil")
                .args(["detach", &self.device])
                .status();
        }
    }

    /// Set an old mtime on a file using `touch -t` (macOS/BSD syntax).
    #[cfg(target_os = "macos")]
    fn set_old_mtime(path: &std::path::Path) {
        let _ = std::process::Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(path)
            .status();
    }

    /// Read the `1K-blocks` (total) column of `df -k` for the mount.
    #[cfg(target_os = "macos")]
    fn ramdisk_total_kb(disk: &RamDisk) -> f64 {
        let out = String::from_utf8_lossy(
            &std::process::Command::new("df")
                .args(["-k", disk.mount.to_str().unwrap()])
                .output()
                .unwrap()
                .stdout,
        )
        .to_string();
        let line = out.lines().nth(1).unwrap();
        line.split_whitespace().nth(1).unwrap().parse::<f64>().unwrap()
    }

    /// Shared driver for the low-disk prune tests.
    ///
    /// Layout on the RAM disk:
    /// - `operator.log` (active, never pruned)
    /// - `operator.log.1`, `operator.log.2.gz` (old mtime — pruned by age)
    /// - `operator.log.3..13` (fresh mtime, sized `file_kb` each — pruned by size)
    /// - `filler.bin` (not a log file — stays forever)
    ///
    /// `target_free_kb` is the free space left after filling (must be < 10%).
    /// Returns the number of fresh log files remaining after pruning.
    #[cfg(target_os = "macos")]
    fn run_prune_on_full_disk(disk: &RamDisk, target_free_kb: u64, file_kb: u64) -> usize {
        // Old log files (pruned in phase 1)
        std::fs::write(disk.mount.join("operator.log"), b"active").unwrap();
        disk.write_file("operator.log.1", 8).unwrap();
        set_old_mtime(&disk.mount.join("operator.log.1"));
        disk.write_file("operator.log.2.gz", 8).unwrap();
        set_old_mtime(&disk.mount.join("operator.log.2.gz"));

        // Fresh log files (phase-2 candidates)
        for i in 3..=13 {
            disk.write_file(&format!("operator.log.{}", i), file_kb).unwrap();
        }

        // Fill the disk so free space lands near the target
        let avail_kb = {
            let out = String::from_utf8_lossy(
                &std::process::Command::new("df")
                    .args(["-k", disk.mount.to_str().unwrap()])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .to_string();
            let line = out.lines().nth(1).unwrap();
            line.split_whitespace().nth(3).unwrap().parse::<f64>().unwrap()
        };
        let filler_kb = (avail_kb - target_free_kb as f64 - 128.0).max(0.0) as u64;
        disk.write_file("filler.bin", filler_kb).unwrap();

        let free_before = disk.free_fraction();
        assert!(
            free_before < DISK_SPACE_WARNING_THRESHOLD,
            "free={} must be below warning threshold",
            free_before
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(prune_logs_if_needed(&disk.mount, 7)).expect("prune ok");

        // Active log and filler untouched
        assert!(disk.mount.join("operator.log").exists());
        assert!(disk.mount.join("filler.bin").exists());
        // Old files removed by age
        assert!(!disk.mount.join("operator.log.1").exists());
        assert!(!disk.mount.join("operator.log.2.gz").exists());

        (3..=13).filter(|i| disk.mount.join(format!("operator.log.{}", i)).exists()).count()
    }

    /// Disk space is fine (fresh RAM disk): prune is a no-op even for old files.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_prune_logs_noop_when_space_ok() {
        let Some(disk) = RamDisk::create(16 * 1024) else {
            eprintln!("skipping: ramdisk unavailable");
            return;
        };
        std::fs::write(disk.mount.join("operator.log"), b"active").unwrap();
        disk.write_file("operator.log.1", 8).unwrap();
        set_old_mtime(&disk.mount.join("operator.log.1"));
        disk.write_file("operator.log.2.gz", 8).unwrap();
        set_old_mtime(&disk.mount.join("operator.log.2.gz"));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(prune_logs_if_needed(&disk.mount, 1)).expect("prune ok");

        assert!(disk.mount.join("operator.log").exists());
        assert!(disk.mount.join("operator.log.1").exists());
        assert!(disk.mount.join("operator.log.2.gz").exists());
    }

    /// Phase 2 hits the re-check reset branch: 5 deletions don't free 10%, the
    /// next re-check after 10 deletions does (break).
    #[cfg(target_os = "macos")]
    #[test]
    fn test_prune_logs_full_disk_reset_then_break() {
        let Some(disk) = RamDisk::create(64 * 1024) else {
            eprintln!("skipping: ramdisk unavailable");
            return;
        };
        let total_kb = ramdisk_total_kb(&disk);
        let file_kb = (0.008 * total_kb) as u64; // 11 files x 0.8% = 8.8% of disk
        let remaining = run_prune_on_full_disk(&disk, (0.04 * total_kb) as u64, file_kb);
        // 10 of 11 fresh files deleted, 1 remains
        assert_eq!(remaining, 1);
    }

    /// Critical free space (< 5%): age threshold overridden to 1 day, phase 2
    /// breaks directly at the first re-check (5 deletions free > 10%).
    #[cfg(target_os = "macos")]
    #[test]
    fn test_prune_logs_full_disk_break_at_five() {
        let Some(disk) = RamDisk::create(64 * 1024) else {
            eprintln!("skipping: ramdisk unavailable");
            return;
        };
        let total_kb = ramdisk_total_kb(&disk);
        let file_kb = (0.03 * total_kb) as u64; // 5 x 3% = 15% > 10% -> break at 5
        let remaining = run_prune_on_full_disk(&disk, (0.04 * total_kb) as u64, file_kb);
        assert_eq!(remaining, 6);
    }

    /// Non-critical (but below warning) disk space: age threshold is the
    /// caller's max_age_days, not the critical 1-day override; re-check breaks
    /// at the first 5-deletion boundary.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_prune_logs_full_disk_non_critical_path() {
        let Some(disk) = RamDisk::create(64 * 1024) else {
            eprintln!("skipping: ramdisk unavailable");
            return;
        };
        let total_kb = ramdisk_total_kb(&disk);
        // free lands between 5% and 10%: critical=false -> caller's max_age used.
        // 5 deletions x 0.8% = 4% on top of ~7.4% free -> above 10% -> break at 5.
        let remaining = run_prune_on_full_disk(&disk, (0.07 * total_kb) as u64, (0.008 * total_kb) as u64);
        assert_eq!(remaining, 6);
    }
}
