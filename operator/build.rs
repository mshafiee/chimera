//! Build script: capture the git commit hash at compile time.
//!
//! Exposes `GIT_HASH` as an environment variable so the operator can stamp
//! every decision record and promotion episode with the exact code revision
//! that produced it (C1 run-scoped evidence). Resolution order:
//!
//! 1. An existing `GIT_HASH` env var (e.g. a Docker `--build-arg GIT_HASH=…`
//!    so the image ships with the real commit even when the builder has no
//!    git checkout).
//! 2. `git rev-parse HEAD` run against the repo (local / CI builds).
//! 3. `"unknown"` fallback when neither is available (source tarball, etc.)
//!    so compilation never fails.

use std::process::Command;

fn main() {
    // Re-run only when the git HEAD changes (new commit / checkout) or the
    // explicit GIT_HASH build arg changes.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
    println!("cargo:rerun-if-env-changed=GIT_HASH");

    let hash = std::env::var("GIT_HASH")
        .ok()
        .filter(|s| !s.is_empty() && s != "unknown")
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={}", hash);
}
