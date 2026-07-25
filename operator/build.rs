//! Build script: capture the git commit hash at compile time.
//!
//! Exposes `GIT_HASH` as an environment variable so the operator can stamp
//! every decision record and promotion episode with the exact code revision
//! that produced it (C1 run-scoped evidence). Falls back to "unknown" when
//! git is unavailable (e.g. building from a source tarball) so compilation
//! never fails.

use std::process::Command;

fn main() {
    // Re-run only when the git HEAD changes (new commit / checkout).
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");

    let hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={}", hash);
}
