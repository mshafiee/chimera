//! Build script: capture the git commit hash at compile time.
//!
//! Exposes `GIT_HASH` as an environment variable so the operator can stamp
//! every decision record and promotion episode with the exact code revision
//! that produced it (C1 run-scoped evidence). Resolution order:
//!
//! 1. An existing `GIT_HASH` env var (e.g. a Docker `--build-arg GIT_HASH=…`
//!    so the image ships with the real commit even when the builder has no
//!    git checkout).
//! 2. `git rev-parse HEAD` run against the repo (local / CI builds), with a
//!    `-dirty` marker appended when the working tree has uncommitted changes.
//! 3. `"unknown"` fallback when neither is available (source tarball, etc.)
//!    so compilation never fails.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate the real git directory: plain clones use `.git/`; git worktrees use a
/// `.git` file whose contents are `gitdir: <path>` (the gitdir may itself be a
/// file pointing at another gitdir).
fn git_dir() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let mut current = PathBuf::from(manifest);
    current.push("../.git");
    for _ in 0..4 {
        if current.is_dir() {
            return Some(current);
        }
        if current.is_file() {
            let contents = std::fs::read_to_string(&current).ok()?;
            let target = contents.strip_prefix("gitdir:")?.trim();
            let target_path = Path::new(target);
            current = if target_path.is_absolute() {
                target_path.to_path_buf()
            } else {
                current.parent()?.join(target_path)
            };
            continue;
        }
        return None;
    }
    None
}

/// Watch the git metadata that moves when HEAD changes. In a plain clone this
/// is `.git/HEAD`; in a worktree HEAD lives under the real gitdir, and once
/// refs are packed by `git gc`/`git pack-refs` the commit is only reachable via
/// `packed-refs`. Watch all of these so the stamped GIT_HASH never silently
/// goes stale after a commit.
fn watch_git_metadata() {
    if let Some(git_dir) = git_dir() {
        for rel in ["HEAD", "packed-refs", "refs/heads"] {
            let path = git_dir.join(rel);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    println!("cargo:rerun-if-env-changed=GIT_HASH");
}

/// True when the working tree has uncommitted modifications relative to HEAD.
fn is_worktree_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

fn main() {
    watch_git_metadata();

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
                .map(|hash| {
                    // The stamp claims to be "the exact code revision that produced
                    // it"; a dirty tree does not correspond to the HEAD commit, so
                    // mark it as such instead of producing misleading evidence.
                    if is_worktree_dirty() {
                        format!("{hash}-dirty")
                    } else {
                        hash
                    }
                })
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Sanitize: a GIT_HASH env value with control characters could inject
    // additional directives into the build output via cargo's key=value parsing.
    let hash = hash.replace(['\n', '\r', '\t'], "");
    println!("cargo:rustc-env=GIT_HASH={}", hash);
}
