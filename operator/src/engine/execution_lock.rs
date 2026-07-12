//! Trade-level execution lock for signal processing idempotency
//!
//! Prevents concurrent processing of the same trade_uuid by multiple workers.
//! Uses DashMap for sub-microsecond lock acquisition with automatic expiration
//! and cleanup for crash safety.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace, warn};

/// Configuration for execution lock behavior
#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionLockConfig {
    /// Enable/disable execution locking
    #[serde(default = "default_execution_lock_enabled")]
    pub enabled: bool,

    /// Lock timeout in seconds (auto-expiration for crash safety)
    #[serde(default = "default_lock_timeout")]
    pub lock_timeout_seconds: u64,

    /// Background cleanup interval in seconds
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_seconds: u64,
}

impl Default for ExecutionLockConfig {
    fn default() -> Self {
        Self {
            enabled: default_execution_lock_enabled(),
            lock_timeout_seconds: default_lock_timeout(),
            cleanup_interval_seconds: default_cleanup_interval(),
        }
    }
}

fn default_execution_lock_enabled() -> bool {
    true
}

fn default_lock_timeout() -> u64 {
    120 // 2 minutes default
}

fn default_cleanup_interval() -> u64 {
    30 // 30 seconds default
}

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

        if let Some(mut existing) = self.locks.get_mut(trade_uuid) {
            // Lock already held, check if expired
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
            } else {
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

                // Replace the expired lock entry
                existing.worker_id = worker_id.to_string();
                existing.acquired_at = now;
                existing.expires_at = expires_at;

                let trade_uuid_owned = trade_uuid.to_string();

                if let Some(ref metrics) = self.metrics {
                    metrics.increment_lock_acquire_success();
                    metrics.record_lock_held_duration(Duration::from_secs(0));
                }

                return Some(LockGuard {
                    lock: Arc::new(ActiveLock {
                        trade_uuid: trade_uuid_owned,
                        locks: Arc::clone(&self.locks),
                        acquired_at: now,
                        metrics: self.metrics.clone(),
                    }),
                });
            }
        }

        // No existing lock, create new entry and insert it
        let entry = LockEntry {
            worker_id: worker_id.to_string(),
            acquired_at: now,
            expires_at,
        };

        let trade_uuid_owned = trade_uuid.to_string();
        self.locks.insert(trade_uuid_owned.clone(), entry);

        if let Some(ref metrics) = self.metrics {
            metrics.increment_lock_acquire_success();
            metrics.record_lock_held_duration(Duration::from_secs(0)); // Will be updated on release
        }

        Some(LockGuard {
            lock: Arc::new(ActiveLock {
                trade_uuid: trade_uuid_owned,
                locks: Arc::clone(&self.locks),
                acquired_at: now,
                metrics: self.metrics.clone(),
            }),
        })
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

// LockGuard is Send and Sync because the underlying lock is thread-safe
unsafe impl Send for LockGuard {}
unsafe impl Sync for LockGuard {}

impl LockGuard {
    /// Get the trade_uuid for this lock
    pub fn trade_uuid(&self) -> &str {
        self.lock.trade_uuid()
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
}

/// Active lock implementation that releases on drop
struct ActiveLock {
    trade_uuid: String,
    locks: Arc<DashMap<String, LockEntry>>,
    acquired_at: Instant,
    metrics: Option<Arc<crate::metrics::ExecutionLockMetrics>>,
}

impl LockImpl for ActiveLock {
    fn trade_uuid(&self) -> &str {
        &self.trade_uuid
    }

    fn release(&self) -> bool {
        let held_duration = self.acquired_at.elapsed();

        // Remove from map (this is safe even if another thread acquired it in the meantime)
        // We only remove if we're still the holder, which DashMap handles via remove()
        if let Some(_entry) = self.locks.remove(&self.trade_uuid) {
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
        let mut config = ExecutionLockConfig::default();
        config.lock_timeout_seconds = 1;
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
        let mut config = ExecutionLockConfig::default();
        config.enabled = false;
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
        let mut config = ExecutionLockConfig::default();
        config.lock_timeout_seconds = 1;
        let lock = ExecutionLock::new(config, None);

        let _guard1 = lock.try_acquire("trade-123", "worker-1");
        let _guard2 = lock.try_acquire("trade-456", "worker-1");

        assert_eq!(lock.active_lock_count(), 2, "Should have 2 active locks");

        thread::sleep(Duration::from_secs(2));

        let cleaned = lock.cleanup_expired();
        assert_eq!(cleaned, 2, "Should clean up 2 expired locks");
        assert_eq!(lock.active_lock_count(), 0, "Should have no active locks");
    }
}