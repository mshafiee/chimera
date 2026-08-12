//! Trade-level execution lock for signal processing idempotency
//!
//! Prevents concurrent processing of the same trade_uuid by multiple workers.
//! Uses DashMap for sub-microsecond lock acquisition with automatic expiration
//! and cleanup for crash safety.

use crate::config::ExecutionLockConfig;
use dashmap::DashMap;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace, warn};

/// Lock entry stored in DashMap
#[derive(Debug, Clone)]
struct LockEntry {
    /// Worker ID that holds this lock
    worker_id: String,
    /// When the lock was acquired
    acquired_at: Instant,
    /// When the lock will expire (crash safety)
    expires_at: Instant,
}

/// Trade execution lock using DashMap for thread-safe, low-latency locking
pub struct ExecutionLock {
    /// Active locks keyed by trade_uuid
    locks: Arc<DashMap<String, LockEntry>>,
    /// Lock configuration
    config: ExecutionLockConfig,
    /// Metrics for monitoring (optional)
    metrics: Option<Arc<crate::metrics::ExecutionLockMetrics>>,
}

impl ExecutionLock {
    /// Create a new execution lock with the given configuration
    pub fn new(config: ExecutionLockConfig, metrics: Option<Arc<crate::metrics::ExecutionLockMetrics>>) -> Self {
        info!(
            enabled = config.enabled,
            timeout_seconds = config.lock_timeout_seconds,
            "Execution lock initialized"
        );

        Self {
            locks: Arc::new(DashMap::new()),
            config,
            metrics,
        }
    }

    /// Attempt to acquire a lock for the given trade_uuid
    ///
    /// Returns None if the lock is already held by another worker (non-blocking).
    /// Returns Some(LockGuard) if the lock was successfully acquired.
    ///
    /// The lock guard automatically releases the lock when dropped (RAII pattern).
    pub fn try_acquire(&self, trade_uuid: &str, worker_id: &str) -> Option<LockGuard> {
        // Fast path: if disabled, always succeed with no-op guard
        if !self.config.enabled {
            trace!(
                trade_uuid = %trade_uuid,
                worker_id = %worker_id,
                "Execution lock disabled, allowing processing"
            );

            if let Some(ref metrics) = self.metrics {
                metrics.increment_lock_acquire_disabled();
            }

            return Some(LockGuard {
                lock: Arc::new(DisabledLock),
            });
        }

        let now = Instant::now();
        let timeout = Duration::from_secs(self.config.lock_timeout_seconds);
        let expires_at = now + timeout;

        // Atomic check-and-insert under a single shard lock: a plain
        // `get_mut` followed by `insert` has a race window where another
        // worker can insert the same trade_uuid in between, silently
        // overwriting its entry and breaking mutual exclusion.
        let trade_uuid_owned = trade_uuid.to_string();
        match self.locks.entry(trade_uuid_owned.clone()) {
            dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                let existing = occ.get_mut();
                if existing.expires_at > now {
                    // Still held by another worker
                    debug!(
                        trade_uuid = %trade_uuid,
                        holder = %existing.worker_id,
                        worker_id = %worker_id,
                        "Lock already held, skipping acquisition"
                    );

                    if let Some(ref metrics) = self.metrics {
                        metrics.increment_lock_acquire_failed();
                    }

                    return None;
                }

                // Expired lock, replace it
                warn!(
                    trade_uuid = %trade_uuid,
                    previous_holder = %existing.worker_id,
                    new_holder = %worker_id,
                    "Replacing expired lock"
                );

                if let Some(ref metrics) = self.metrics {
                    metrics.increment_lock_expired_reclaimed();
                }

                // Record the previous holder's actual held duration (not 0).
                if let Some(ref metrics) = self.metrics {
                    metrics.increment_lock_acquire_success();
                    metrics.record_lock_held_duration(now - existing.acquired_at);
                }

                existing.worker_id = worker_id.to_string();
                existing.acquired_at = now;
                existing.expires_at = expires_at;

                Some(LockGuard {
                    lock: Arc::new(ActiveLock {
                        trade_uuid: trade_uuid_owned,
                        locks: Arc::clone(&self.locks),
                        acquired_at: now,
                        worker_id: worker_id.to_string(),
                        timeout,
                        metrics: self.metrics.clone(),
                    }),
                })
            }
            dashmap::mapref::entry::Entry::Vacant(v) => {
                v.insert(LockEntry {
                    worker_id: worker_id.to_string(),
                    acquired_at: now,
                    expires_at,
                });

                if let Some(ref metrics) = self.metrics {
                    metrics.increment_lock_acquire_success();
                    // Held duration is recorded on release, not here.
                }

                Some(LockGuard {
                    lock: Arc::new(ActiveLock {
                        trade_uuid: trade_uuid_owned,
                        locks: Arc::clone(&self.locks),
                        acquired_at: now,
                        worker_id: worker_id.to_string(),
                        timeout,
                        metrics: self.metrics.clone(),
                    }),
                })
            }
        }
    }

    /// Extend the expiry of a still-held lock (heartbeat for long-running
    /// processing). Returns false when this worker is no longer the holder.
    pub fn renew(&self, trade_uuid: &str, worker_id: &str) -> bool {
        let now = Instant::now();
        let timeout = Duration::from_secs(self.config.lock_timeout_seconds);
        if let Some(mut entry) = self.locks.get_mut(trade_uuid) {
            if entry.worker_id == worker_id {
                entry.expires_at = now + timeout;
                return true;
            }
        }
        false
    }

    /// Force release a lock (for recovery scenarios)
    ///
    /// This should only be used by the recovery manager when handling stuck positions.
    pub fn force_release(&self, trade_uuid: &str) {
        if let Some((_key, entry)) = self.locks.remove(trade_uuid) {
            warn!(
                trade_uuid = %trade_uuid,
                holder = %entry.worker_id,
                held_duration_secs = entry.acquired_at.elapsed().as_secs(),
                "Force releasing lock"
            );

            if let Some(ref metrics) = self.metrics {
                metrics.increment_lock_force_released();
            }
        }
    }

    /// Clean up expired locks (background task)
    ///
    /// Should be called periodically by a background task to reclaim locks from crashed workers.
    pub fn cleanup_expired(&self) -> usize {
        let now = Instant::now();
        let mut cleaned = 0;

        self.locks.retain(|trade_uuid, entry| {
            if entry.expires_at <= now {
                let held_duration = entry.acquired_at.elapsed();
                debug!(
                    trade_uuid = %trade_uuid,
                    worker_id = %entry.worker_id,
                    held_duration_secs = held_duration.as_secs(),
                    "Cleaning up expired lock"
                );

                cleaned += 1;

                if let Some(ref metrics) = self.metrics {
                    metrics.increment_lock_expired_cleaned();
                }

                false // Remove from map
            } else {
                true // Keep in map
            }
        });

        if cleaned > 0 {
            info!(
                cleaned = cleaned,
                active_locks = self.locks.len(),
                "Cleanup completed"
            );
        }

        cleaned
    }

    /// Get current number of active locks
    pub fn active_lock_count(&self) -> usize {
        self.locks.len()
    }

    /// Check if a specific trade_uuid is currently locked
    pub fn is_locked(&self, trade_uuid: &str) -> bool {
        if let Some(entry) = self.locks.get(trade_uuid) {
            let now = Instant::now();
            entry.expires_at > now
        } else {
            false
        }
    }

    /// Get lock information for debugging/monitoring
    pub fn get_lock_info(&self, trade_uuid: &str) -> Option<LockInfo> {
        if let Some(entry) = self.locks.get(trade_uuid) {
            let now = Instant::now();
            if entry.expires_at > now {
                Some(LockInfo {
                    trade_uuid: trade_uuid.to_string(),
                    worker_id: entry.worker_id.clone(),
                    held_duration: entry.acquired_at.elapsed(),
                    time_until_expiry: entry.expires_at.saturating_duration_since(now),
                })
            } else {
                None // Lock expired
            }
        } else {
            None
        }
    }

    /// Get all active locks (for monitoring/debugging)
    pub fn get_all_locks(&self) -> Vec<LockInfo> {
        let now = Instant::now();
        self.locks
            .iter()
            .filter(|entry| entry.value().expires_at > now)
            .map(|entry| LockInfo {
                trade_uuid: entry.key().clone(),
                worker_id: entry.value().worker_id.clone(),
                held_duration: entry.value().acquired_at.elapsed(),
                time_until_expiry: entry.value().expires_at.saturating_duration_since(now),
            })
            .collect()
    }
}

/// Lock information for debugging/monitoring
#[derive(Debug, Clone, Serialize)]
pub struct LockInfo {
    pub trade_uuid: String,
    pub worker_id: String,
    pub held_duration: Duration,
    pub time_until_expiry: Duration,
}

/// RAII guard for automatic lock release
///
/// When this guard is dropped (goes out of scope or panics), the lock is automatically released.
pub struct LockGuard {
    lock: Arc<dyn LockImpl>,
}

impl LockGuard {
    /// Get the trade_uuid for this lock
    pub fn trade_uuid(&self) -> &str {
        self.lock.trade_uuid()
    }

    /// Heartbeat: extend this lock's expiry while processing continues.
    pub fn renew(&self) -> bool {
        self.lock.renew()
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        self.lock.release();
    }
}

/// Trait for lock implementations (active vs disabled)
trait LockImpl: Send + Sync {
    /// Get the trade_uuid for this lock (for debugging)
    fn trade_uuid(&self) -> &str;

    /// Release the lock
    fn release(&self) -> bool;

    /// Extend the lock expiry (no-op for the disabled lock)
    fn renew(&self) -> bool {
        false
    }
}

/// Active lock implementation that releases on drop
struct ActiveLock {
    trade_uuid: String,
    locks: Arc<DashMap<String, LockEntry>>,
    acquired_at: Instant,
    worker_id: String,
    timeout: Duration,
    metrics: Option<Arc<crate::metrics::ExecutionLockMetrics>>,
}

impl LockImpl for ActiveLock {
    fn trade_uuid(&self) -> &str {
        &self.trade_uuid
    }

    fn release(&self) -> bool {
        let held_duration = self.acquired_at.elapsed();

        // Only remove the lock if this guard is still the current holder;
        // otherwise a stale guard (whose lock expired, was force-released, or
        // cleaned up and then re-acquired by another worker) would delete the
        // new holder's lock, allowing a third worker to acquire concurrently.
        let still_holder = self
            .locks
            .get(&self.trade_uuid)
            .map(|e| e.worker_id == self.worker_id)
            .unwrap_or(false);

        if still_holder && self.locks.remove(&self.trade_uuid).is_some() {
            trace!(
                trade_uuid = %self.trade_uuid,
                held_duration_secs = held_duration.as_secs_f64(),
                "Lock released"
            );

            if let Some(ref metrics) = self.metrics {
                metrics.increment_lock_released();
                metrics.record_lock_held_duration(held_duration);
            }
            true
        } else {
            false
        }
    }

    fn renew(&self) -> bool {
        if let Some(mut entry) = self.locks.get_mut(&self.trade_uuid) {
            if entry.worker_id == self.worker_id {
                entry.expires_at = Instant::now() + self.timeout;
                return true;
            }
        }
        false
    }
}

impl Drop for ActiveLock {
    fn drop(&mut self) {
        // This is a no-op since release() is called via LockGuard::drop()
        // Keeping this for safety in case LockGuard is not used correctly
    }
}

/// No-op lock implementation for when locking is disabled
struct DisabledLock;

impl LockImpl for DisabledLock {
    fn trade_uuid(&self) -> &str {
        "disabled"
    }

    fn release(&self) -> bool {
        // No-op for disabled locks
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_lock_acquisition_success() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let guard = lock.try_acquire("trade-123", "worker-1");
        assert!(guard.is_some(), "Should successfully acquire lock");

        assert!(lock.is_locked("trade-123"), "Trade should be locked");
    }

    #[test]
    fn test_lock_acquisition_failure() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let guard1 = lock.try_acquire("trade-123", "worker-1");
        assert!(guard1.is_some(), "First acquisition should succeed");

        let guard2 = lock.try_acquire("trade-123", "worker-2");
        assert!(guard2.is_none(), "Second acquisition should fail");
    }

    #[test]
    fn test_lock_automatic_release() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let guard = lock.try_acquire("trade-123", "worker-1");
        assert!(lock.is_locked("trade-123"), "Should be locked while guard is active");

        // Explicitly drop the guard
        drop(guard);

        // Lock should be released after guard is dropped
        assert!(!lock.is_locked("trade-123"), "Lock should be released after guard drop");
    }

    #[test]
    fn test_lock_expiration() {
        let config = ExecutionLockConfig {
            enabled: true,
            lock_timeout_seconds: 1,
            cleanup_interval_seconds: 30,
        };
        let lock = ExecutionLock::new(config, None);

        let _guard1 = lock.try_acquire("trade-123", "worker-1");
        assert!(lock.is_locked("trade-123"), "Initially locked");

        thread::sleep(Duration::from_secs(2));

        // After expiration, a new worker should be able to acquire
        let guard2 = lock.try_acquire("trade-123", "worker-2");
        assert!(guard2.is_some(), "Should acquire expired lock");
    }

    #[test]
    fn test_disabled_lock() {
        let config = ExecutionLockConfig {
            enabled: false,
            lock_timeout_seconds: 120,
            cleanup_interval_seconds: 30,
        };
        let lock = ExecutionLock::new(config, None);

        let guard1 = lock.try_acquire("trade-123", "worker-1");
        let guard2 = lock.try_acquire("trade-123", "worker-2");

        // Both should succeed when disabled
        assert!(guard1.is_some(), "First acquisition should succeed");
        assert!(guard2.is_some(), "Second acquisition should succeed when disabled");
    }

    #[test]
    fn test_force_release() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let _guard = lock.try_acquire("trade-123", "worker-1");
        assert!(lock.is_locked("trade-123"), "Should be locked");

        lock.force_release("trade-123");
        assert!(!lock.is_locked("trade-123"), "Lock should be force-released");
    }

    #[test]
    fn test_cleanup_expired() {
        let config = ExecutionLockConfig {
            enabled: true,
            lock_timeout_seconds: 1,
            cleanup_interval_seconds: 30,
        };
        let lock = ExecutionLock::new(config, None);

        let _guard1 = lock.try_acquire("trade-123", "worker-1");
        let _guard2 = lock.try_acquire("trade-456", "worker-1");

        assert_eq!(lock.active_lock_count(), 2, "Should have 2 active locks");

        thread::sleep(Duration::from_secs(2));

        let cleaned = lock.cleanup_expired();
        assert_eq!(cleaned, 2, "Should clean up 2 expired locks");
        assert_eq!(lock.active_lock_count(), 0, "Should have no active locks");
    }

    // ==========================================================================
    // ADDITIONAL COVERAGE
    // ==========================================================================

    fn metrics() -> Arc<crate::metrics::ExecutionLockMetrics> {
        Arc::new(crate::metrics::ExecutionLockMetrics::new(
            &prometheus::Registry::new(),
        ))
    }

    #[test]
    fn test_renew_returns_true_for_holder_false_for_other() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let _guard = lock.try_acquire("trade-123", "worker-1").unwrap();
        assert!(lock.renew("trade-123", "worker-1"), "holder can renew");
        assert!(
            !lock.renew("trade-123", "worker-2"),
            "non-holder cannot renew"
        );
        assert!(
            !lock.renew("missing", "worker-1"),
            "renewing a missing lock returns false"
        );
    }

    #[test]
    fn test_guard_renew_heartbeats_lock() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let guard = lock.try_acquire("trade-123", "worker-1").unwrap();
        assert!(guard.renew(), "guard heartbeat extends expiry");
        assert_eq!(guard.trade_uuid(), "trade-123");
    }

    #[test]
    fn test_get_lock_info_active_and_missing() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        assert!(lock.get_lock_info("missing").is_none());

        let _guard = lock.try_acquire("trade-123", "worker-1").unwrap();
        let info = lock.get_lock_info("trade-123").expect("active lock");
        assert_eq!(info.trade_uuid, "trade-123");
        assert_eq!(info.worker_id, "worker-1");
    }

    #[test]
    fn test_get_lock_info_expired_returns_none() {
        let config = ExecutionLockConfig {
            enabled: true,
            lock_timeout_seconds: 1,
            cleanup_interval_seconds: 30,
        };
        let lock = ExecutionLock::new(config, None);

        let _guard = lock.try_acquire("trade-123", "worker-1").unwrap();
        thread::sleep(Duration::from_secs(2));
        assert!(
            lock.get_lock_info("trade-123").is_none(),
            "expired lock should not be reported"
        );
    }

    #[test]
    fn test_get_all_locks_returns_only_active() {
        let config = ExecutionLockConfig {
            enabled: true,
            lock_timeout_seconds: 1,
            cleanup_interval_seconds: 30,
        };
        let lock = ExecutionLock::new(config, None);

        let _g1 = lock.try_acquire("trade-a", "w1").unwrap();
        let _g2 = lock.try_acquire("trade-b", "w1").unwrap();
        assert_eq!(lock.get_all_locks().len(), 2);

        thread::sleep(Duration::from_secs(2));
        assert_eq!(
            lock.get_all_locks().len(),
            0,
            "expired locks are excluded from all_locks"
        );
    }

    #[test]
    fn test_metrics_acquire_success_and_failed() {
        let config = ExecutionLockConfig::default();
        let m = metrics();
        let lock = ExecutionLock::new(config, Some(m.clone()));

        let g1 = lock.try_acquire("trade-123", "w1");
        assert!(g1.is_some());
        assert_eq!(m.acquire_success.get(), 1);

        let g2 = lock.try_acquire("trade-123", "w2");
        assert!(g2.is_none());
        assert_eq!(m.acquire_failed.get(), 1);
    }

    #[test]
    fn test_metrics_disabled_increments_disabled() {
        let config = ExecutionLockConfig {
            enabled: false,
            lock_timeout_seconds: 120,
            cleanup_interval_seconds: 30,
        };
        let m = metrics();
        let lock = ExecutionLock::new(config, Some(m.clone()));

        let guard = lock.try_acquire("trade-123", "w1");
        assert!(guard.is_some());
        assert_eq!(m.acquire_disabled.get(), 1);
    }

    #[test]
    fn test_metrics_release_and_force_release() {
        let config = ExecutionLockConfig::default();
        let m = metrics();
        let lock = ExecutionLock::new(config, Some(m.clone()));

        let guard = lock.try_acquire("trade-123", "w1");
        drop(guard);
        assert_eq!(m.released.get(), 1, "guard release increments released");

        let _guard = lock.try_acquire("trade-123", "w1");
        lock.force_release("trade-123");
        assert_eq!(m.force_released.get(), 1);
    }

    #[test]
    fn test_stale_guard_does_not_release_new_holder() {
        let config = ExecutionLockConfig::default();
        let lock = ExecutionLock::new(config, None);

        let guard1 = lock.try_acquire("trade-123", "worker-1").unwrap();
        // The first holder's lock is force-released while guard1 is still alive.
        lock.force_release("trade-123");
        // A second worker re-acquires the same uuid.
        let _guard2 = lock.try_acquire("trade-123", "worker-2").unwrap();
        // Dropping the STALE guard must not delete the new holder's lock.
        drop(guard1);
        assert!(
            lock.is_locked("trade-123"),
            "stale guard must not release the new holder's lock"
        );
    }

    #[test]
    fn test_disabled_guard_is_noop() {
        let config = ExecutionLockConfig {
            enabled: false,
            lock_timeout_seconds: 120,
            cleanup_interval_seconds: 30,
        };
        let lock = ExecutionLock::new(config, None);

        let guard = lock.try_acquire("trade-123", "w1").unwrap();
        assert_eq!(guard.trade_uuid(), "disabled");
        assert!(!guard.renew(), "disabled lock renew is a no-op");
        drop(guard);
        // Nothing was ever inserted.
        assert_eq!(lock.active_lock_count(), 0);
    }

    #[test]
    fn test_expired_lock_reclaimed_with_metrics() {
        let config = ExecutionLockConfig {
            enabled: true,
            lock_timeout_seconds: 1,
            cleanup_interval_seconds: 30,
        };
        let m = metrics();
        let lock = ExecutionLock::new(config, Some(m.clone()));

        let _g1 = lock.try_acquire("trade-123", "w1").unwrap();
        thread::sleep(Duration::from_secs(2));

        // Second acquisition reclaims the expired entry.
        let g2 = lock.try_acquire("trade-123", "w2");
        assert!(g2.is_some());
        assert_eq!(m.expired_reclaimed.get(), 1);
    }

    #[test]
    fn test_cleanup_expired_with_metrics() {
        let config = ExecutionLockConfig {
            enabled: true,
            lock_timeout_seconds: 1,
            cleanup_interval_seconds: 30,
        };
        let m = metrics();
        let lock = ExecutionLock::new(config, Some(m.clone()));

        let _g1 = lock.try_acquire("trade-123", "w1").unwrap();
        thread::sleep(Duration::from_secs(2));

        assert_eq!(lock.cleanup_expired(), 1);
        assert_eq!(m.expired_cleaned.get(), 1);
    }
}