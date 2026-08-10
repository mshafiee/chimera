//! `import_keypair` — import a Solana trading keypair into the encrypted vault.
//!
//! Reads a keypair from stdin (or `--keypair-file`), auto-detects its format
//! (Solana CLI JSON / base58 / hex), validates it, and stores it — along with
//! the current `CHIMERA_SECURITY__WEBHOOK_SECRET` — into `config/secrets.enc`.

//! # Why this exists
//!
//! The operator's normal container mount (`./config:/app/config:ro` in
//! `docker-compose.yml`) is read-only, so the running operator cannot write
//! the vault file itself. This tool is meant to be run as a one-off
//! `docker compose run -v /opt/chimera/config:/app/config:rw` override.
//!
//! # Safety
//!
//! - The keypair is **never** accepted as a CLI argument (would be visible in
//!   `ps`). It is read from stdin or a path.
//! - The input buffer is explicitly zeroized after decoding.
//! - All log output redacts secrets; only the derived base58 pubkey and vault
//!   path are printed.
//! - After writing, the tool immediately re-opens the vault and round-trips
//!   through `load_wallet_keypair`; if anything fails the prior vault is
//!   restored from a `.enc.bak` backup (or removed if no prior vault existed)
//!   so the operator never starts against a corrupt vault.
//!
//! # Example
//!
//! ```bash
//! # Dry-run first (no write):
//! docker compose ... run --rm operator import_keypair --dry-run < id.json
//!
//! # Real import:
//! docker compose ... run --rm operator import_keypair < id.json
//! ```

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use solana_sdk::signature::{Keypair, Signer};
use zeroize::Zeroize;

use crate::engine::transaction_builder::load_wallet_keypair;
use crate::keypair_utils::normalize_to_64_bytes;
use crate::vault::{Vault, VaultError, VaultSecrets};

/// Default vault path when neither `--vault-path` nor `$CHIMERA_VAULT_PATH`
/// is provided. Matches the canonical path used by
/// `vault::load_secrets_with_fallback`.
const DEFAULT_VAULT_PATH: &str = "config/secrets.enc";

/// Import a Solana keypair into the encrypted vault (`config/secrets.enc`).
#[derive(Parser, Debug)]
#[command(
    name = "import_keypair",
    about = "Import a Solana keypair into the encrypted vault (config/secrets.enc)"
)]
pub struct Cli {
    /// Read the keypair from this file instead of stdin.
    ///
    /// The file is never accepted as a CLI value to avoid leaking the keypair
    /// through `ps`; only the path is. The contents are read, normalized,
    /// validated, and the in-memory buffer is zeroized.
    #[arg(long)]
    keypair_file: Option<PathBuf>,

    /// Vault output path. Defaults to `$CHIMERA_VAULT_PATH` or
    /// `config/secrets.enc`.
    #[arg(long)]
    vault_path: Option<PathBuf>,

    /// Validate, derive the pubkey, and print the planned vault contents
    /// (secrets redacted) without writing.
    #[arg(long)]
    dry_run: bool,
}

pub fn main() -> Result<()> {
    run(Cli::parse())
}

/// The import pipeline. Split out of `main()` so tests can drive it with a
/// constructed [`Cli`] (via `parse_from`) and controlled env/temp paths.
pub fn run(cli: Cli) -> Result<()> {
    // 1. Resolve vault key from env. Hard-fail with actionable guidance
    //    rather than auto-generating — the key must outlive this process.
    let vault = Vault::from_env().map_err(|e| {
        anyhow!(
            "{}\n\n\
             Generate one now with:  openssl rand -hex 32\n\
             Then append to your .env:\n  CHIMERA_VAULT_KEY=<hex>\n\
             (and re-run this tool in a shell that has that env var loaded)",
            e
        )
    })?;

    // 2. Resolve the webhook secret. Refuse to build a vault that would
    //    silently break Helius HMAC verification on the next inbound webhook.
    let webhook_secret = std::env::var("CHIMERA_SECURITY__WEBHOOK_SECRET")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "CHIMERA_SECURITY__WEBHOOK_SECRET is not set or empty.\n\
                 A vault without a webhook secret would break inbound webhook \
                 HMAC verification. Set CHIMERA_SECURITY__WEBHOOK_SECRET in \
                 your .env before importing the keypair."
            )
        })?;

    // 3. Resolve vault path: explicit flag > env var > default.
    let vault_path = cli
        .vault_path
        .clone()
        .or_else(|| std::env::var("CHIMERA_VAULT_PATH").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_VAULT_PATH));

    // 4. Read the keypair (stdin or file) into a buffer we control.
    let mut keypair_input = read_keypair_input(cli.keypair_file.as_deref())?;

    // 5. Normalize + validate. The 64-byte buffer is what we keep; the
    //    original textual input is zeroized immediately afterwards.
    let keypair_bytes_64 = match normalize_to_64_bytes(&keypair_input) {
        Ok(b) => b,
        Err(e) => {
            keypair_input.zeroize();
            return Err(anyhow!("Keypair validation failed: {}", e));
        }
    };
    keypair_input.zeroize();

    // 6. Confirm the bytes are a real Ed25519 keypair (pubkey matches secret).
    let keypair = Keypair::try_from(keypair_bytes_64.as_slice())
        .map_err(|e| anyhow!("Decoded bytes are not a valid Ed25519 keypair: {:?}", e))?;
    // Keypair::try_from only splits the 64 bytes; depending on the SDK version
    // it may not re-derive the pubkey from the secret. Sign/verify a probe
    // message to deterministically reject a keypair whose secret and public
    // halves disagree.
    let probe = b"import_keypair: secret/public consistency probe";
    let sig = keypair.sign_message(probe);
    if !sig.verify(keypair.pubkey().as_ref(), probe) {
        bail!("Keypair secret and public halves are inconsistent — refusing to import");
    }
    let derived_pubkey = keypair.pubkey();
    let derived_pubkey_b58 = bs58::encode(derived_pubkey.as_ref()).into_string();
    // The Keypair is only needed for pubkey derivation; release it now so the
    // raw secret does not stay resident for the remainder of the process.
    drop(keypair);

    println!("Derived pubkey: {}", derived_pubkey_b58);

    // 7. Load existing vault if present (preserve all fields), else start empty.
    let mut secrets = if vault_path.exists() {
        match vault.load_secrets(&vault_path) {
            Ok(existing) => {
                println!(
                    "Loaded existing vault at {} (preserving all non-overwritten fields)",
                    vault_path.display()
                );
                existing
            }
            Err(VaultError::FileError(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                fresh_secrets(&webhook_secret)
            }
            Err(e) => {
                bail!(
                    "Vault file exists at {} but could not be decrypted with the \
                     current CHIMERA_VAULT_KEY: {}.\n\
                     Aborting to avoid overwriting a vault you can no longer read.",
                    vault_path.display(),
                    e
                );
            }
        }
    } else {
        println!(
            "No existing vault at {} — creating a new one.",
            vault_path.display()
        );
        fresh_secrets(&webhook_secret)
    };

    // 8. Always (re)stamp the webhook secret in case it was rotated in .env
    //    since the last import. When the secret CHANGES, the old current
    //    secret moves to webhook_secret_previous so webhooks still signed with
    //    it verify during the grace period (HMAC checks both). A prior
    //    CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS env var is honored too.
    if secrets.webhook_secret != webhook_secret {
        secrets.webhook_secret_previous = Some(secrets.webhook_secret.clone());
        secrets.webhook_secret = webhook_secret;
    }
    if let Ok(prev) = std::env::var("CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS") {
        let trimmed = prev.trim();
        if !trimmed.is_empty() {
            secrets.webhook_secret_previous = Some(trimmed.to_string());
        }
    }
    if let Ok(rpc_key) = std::env::var("HELIUS_API_KEY") {
        let trimmed = rpc_key.trim();
        if !trimmed.is_empty() {
            secrets.rpc_api_key = Some(trimmed.to_string());
        }
    }
    if let Ok(fb) = std::env::var("CHIMERA_RPC__FALLBACK_API_KEY") {
        let trimmed = fb.trim();
        if !trimmed.is_empty() {
            secrets.fallback_rpc_api_key = Some(trimmed.to_string());
        }
    }

    // 9. Overwrite the wallet key with the hex-canonical form.
    secrets.wallet_private_key = Some(hex::encode(keypair_bytes_64.as_slice()));

    // 10. Print the plan (redacted). In dry-run mode, exit here.
    print_plan(&vault_path, &secrets, &derived_pubkey_b58, cli.dry_run);
    if cli.dry_run {
        println!("\n--dry-run set: vault NOT written. Exiting.");
        return Ok(());
    }

    // 11. Pre-write in-memory round-trip. Catches the vast majority of failure
    //     cases (bad serialization, keypair load regression) WITHOUT touching
    //     disk — so a rotation can't destroy a prior vault if the new bundle
    //     is broken.
    preflight_roundtrip(&vault, &secrets)
        .context("Pre-write validation failed — vault NOT written, prior state intact")?;

    // 12. Back up any existing vault so we can restore on failure. The backup
    //     is created with 0600 to avoid leaking the old ciphertext.
    let backup_path = vault_path.with_extension("enc.bak");
    let had_prior_vault = vault_path.exists();
    if had_prior_vault {
        copy_file_0600(&vault_path, &backup_path).with_context(|| {
            format!(
                "Failed to back up existing vault to {}",
                backup_path.display()
            )
        })?;
    }

    // 13. Atomic write (tmp 0600 + rename).
    if let Err(e) = vault.save_secrets(&secrets, &vault_path) {
        let outcome = restore_backup(&backup_path, &vault_path, had_prior_vault);
        if matches!(outcome, RestoreOutcome::FailedBackupIntact) {
            eprintln!(
                "WARNING: save_secrets failed AND restore from backup failed. \
                 The prior good vault is still at {} (mode 0600). \
                 Manually recover with: cp {} {}",
                backup_path.display(),
                backup_path.display(),
                vault_path.display()
            );
        }
        return Err(e)
            .with_context(|| format!("Failed to write vault to {}", vault_path.display()));
    }
    println!(
        "Vault written: {} (mode 0600, atomic rename)",
        vault_path.display()
    );

    // 14. Post-write round-trip: re-open from disk and confirm load_wallet_keypair
    //     succeeds. This catches the rare case where save succeeded but the
    //     file is somehow unreadable (filesystem fault, race). If it fails we
    //     restore the prior vault from backup rather than deleting — deleting
    //     would lose the old keypair that save_secrets already destroyed.
    if let Err(e) = roundtrip_validate(&vault, &vault_path, &derived_pubkey_b58) {
        let outcome = restore_backup(&backup_path, &vault_path, had_prior_vault);
        let recovery_msg = match outcome {
            RestoreOutcome::Restored => format!(
                "Prior vault RESTORED from backup. \
                 The new keypair was NOT applied to {} — investigate and re-run.",
                vault_path.display()
            ),
            RestoreOutcome::NoPrior => format!(
                "No prior vault existed to restore. The (possibly corrupt) vault \
                 file at {} has been removed.",
                vault_path.display()
            ),
            RestoreOutcome::FailedBackupIntact => format!(
                "RESTORE FAILED — the corrupt vault remains at {}. \
                 The prior good vault is still recoverable at {} (mode 0600). \
                 Manually recover with: cp {} {}\n\
                 Do NOT re-run import_keypair without recovering the backup first.",
                vault_path.display(),
                backup_path.display(),
                backup_path.display(),
                vault_path.display()
            ),
        };
        bail!(
            "Round-trip validation FAILED after write: {}.\n{}",
            e,
            recovery_msg
        );
    }
    println!("Round-trip validation OK — vault decrypts and keypair loads.");

    // 15. Success — remove the backup. Warn loudly if removal fails so the
    //     operator can shred the old keypair manually (important if the
    //     rotation was triggered by a key compromise).
    if had_prior_vault {
        if let Err(rm_err) = std::fs::remove_file(&backup_path) {
            eprintln!(
                "WARNING: failed to remove vault backup at {} ({}). \
                 The prior keypair is still present on disk — shred manually if \
                 this rotation was due to a compromise: shred -u {}",
                backup_path.display(),
                rm_err,
                backup_path.display()
            );
        }
    }

    // 16. Perms sanity (POSIX only). save_secrets already uses 0600 on the
    //     renamed file via OpenOptions; this is a defensive read-back.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&vault_path) {
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                eprintln!(
                    "WARNING: vault file mode is {:04o} — group/other bits set. \
                     Expected 0600. Tighten with: chmod 600 {}",
                    mode,
                    vault_path.display()
                );
            }
        }
    }

    println!("\nDone. Derived trading pubkey: {}", derived_pubkey_b58);
    Ok(())
}

/// Validate the secrets bundle entirely in memory (no disk I/O).
///
/// Encrypts → decrypts → runs `load_wallet_keypair`. If this fails, the
/// vault file is not touched, so a rotation can never destroy a prior vault
/// due to a bad secrets bundle.
fn preflight_roundtrip(vault: &Vault, secrets: &VaultSecrets) -> Result<()> {
    let ciphertext = vault
        .encrypt_secrets(secrets)
        .context("encrypt_secrets failed during pre-write validation")?;
    let decrypted = vault
        .decrypt_secrets(&ciphertext)
        .context("decrypt_secrets failed during pre-write validation")?;
    load_wallet_keypair(&decrypted)
        .context("load_wallet_keypair failed during pre-write validation")?;
    Ok(())
}

/// Copy `src` to `dst` with mode 0600 (POSIX). Used to back up the prior vault.
fn copy_file_0600(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let data = std::fs::read(src)?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(dst)?;
        f.write_all(&data)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(src, dst)?;
        Ok(())
    }
}

/// Outcome of a restore attempt — distinguishes the three possible end states
/// so the caller can print an accurate recovery message.
enum RestoreOutcome {
    /// No prior vault existed; the (possibly corrupt) target was removed.
    NoPrior,
    /// Prior vault existed and was successfully restored from backup.
    Restored,
    /// Prior vault existed but restore failed. The good backup is still
    /// intact at the backup path (mode 0600); the corrupt vault remains at
    /// the target. The operator must manually recover.
    FailedBackupIntact,
}

/// Restore the vault from backup if one exists.
fn restore_backup(
    backup: &std::path::Path,
    target: &std::path::Path,
    had_prior: bool,
) -> RestoreOutcome {
    if !had_prior {
        // No prior vault — just remove whatever (if anything) was written.
        let _ = std::fs::remove_file(target);
        let _ = std::fs::remove_file(backup);
        return RestoreOutcome::NoPrior;
    }
    match std::fs::rename(backup, target) {
        Ok(()) => RestoreOutcome::Restored,
        Err(_) => {
            // rename failed — try copy + remove as a fallback.
            if copy_file_0600(backup, target).is_ok() {
                if let Err(rm_err) = std::fs::remove_file(backup) {
                    eprintln!(
                        "WARNING: restored vault from backup copy but failed to \
                         remove the backup at {} ({}). Shred manually if this \
                         rotation was due to a compromise: shred -u {}",
                        backup.display(),
                        rm_err,
                        backup.display()
                    );
                }
                RestoreOutcome::Restored
            } else {
                // Both rename and copy failed. The backup is still intact at
                // its original path; the corrupt vault is still at target.
                RestoreOutcome::FailedBackupIntact
            }
        }
    }
}

/// Build a fresh `VaultSecrets` with only the webhook secret populated.
fn fresh_secrets(webhook_secret: &str) -> VaultSecrets {
    VaultSecrets {
        webhook_secret: webhook_secret.to_string(),
        webhook_secret_previous: None,
        wallet_private_key: None,
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    }
}

/// Read the keypair from `--keypair-file` or stdin into an owned String.
fn read_keypair_input(path: Option<&std::path::Path>) -> Result<String> {
    let mut buf = String::new();
    match path {
        Some(p) => {
            let mut f = std::fs::File::open(p)
                .with_context(|| format!("Failed to open keypair file {}", p.display()))?;
            f.read_to_string(&mut buf)
                .with_context(|| format!("Failed to read keypair file {}", p.display()))?;
        }
        None => {
            // TTY check so we can print a helpful prompt when interactive.
            if std::io::stdin().is_terminal() {
                eprintln!("Reading keypair from stdin (paste JSON/base58/hex, then Ctrl-D):");
            }
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("Failed to read keypair from stdin")?;
        }
    }
    Ok(buf)
}

/// Print the planned vault contents with all secret material redacted.
fn print_plan(
    vault_path: &std::path::Path,
    secrets: &VaultSecrets,
    pubkey_b58: &str,
    dry_run: bool,
) {
    println!(
        "\n{}",
        if dry_run {
            "=== Dry-run plan ==="
        } else {
            "=== Vault plan ==="
        }
    );
    println!("  vault path:           {}", vault_path.display());
    println!("  wallet pubkey:        {}", pubkey_b58);
    println!(
        "  wallet_private_key:   [REDACTED, {} hex chars]",
        secrets
            .wallet_private_key
            .as_ref()
            .map(|s| s.len())
            .unwrap_or(0)
    );
    println!(
        "  webhook_secret:       [REDACTED, {} chars]",
        secrets.webhook_secret.len()
    );
    println!(
        "  webhook_secret_prev:  [{}]",
        match &secrets.webhook_secret_previous {
            Some(s) => format!("REDACTED, {} chars", s.len()),
            None => "none".to_string(),
        }
    );
    println!(
        "  rpc_api_key:          [{}]",
        match &secrets.rpc_api_key {
            Some(s) => format!("REDACTED, {} chars", s.len()),
            None => "none".to_string(),
        }
    );
    println!(
        "  fallback_rpc_api_key: [{}]",
        match &secrets.fallback_rpc_api_key {
            Some(s) => format!("REDACTED, {} chars", s.len()),
            None => "none".to_string(),
        }
    );
}

/// Re-open the vault and confirm the keypair loads end-to-end.
///
/// The loaded pubkey is compared against the pubkey captured BEFORE the write
/// (`expected_pubkey_b58`) — re-deriving it from the reloaded vault would be
/// tautological (the same pipeline produced both values) and could never catch
/// a wrong-but-self-consistent write.
fn roundtrip_validate(
    vault: &Vault,
    vault_path: &std::path::Path,
    expected_pubkey_b58: &str,
) -> Result<()> {
    let reloaded = vault
        .load_secrets(vault_path)
        .context("Re-open failed: vault could not be decrypted with current key")?;
    let kp =
        load_wallet_keypair(&reloaded).context("load_wallet_keypair failed on reloaded vault")?;
    // Sanity: pubkey must match what we derived before writing.
    let reloaded_b58 = bs58::encode(kp.pubkey().as_ref()).into_string();
    if reloaded_b58 != expected_pubkey_b58 {
        bail!(
            "pubkey mismatch after round-trip: expected {}, got {}",
            expected_pubkey_b58,
            reloaded_b58
        );
    }
    Ok(())
}

// `VaultSecrets` is `pub` with all-`pub` fields so we can construct it above
// without re-declaring it. The Debug impl in vault.rs already redacts fields.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use solana_sdk::signature::{Keypair, Signer};
    use tempfile::NamedTempFile;

    /// End-to-end: write a fresh vault via the same path the binary uses,
    /// then re-open + load_wallet_keypair, and confirm pubkey matches.
    #[test]
    fn save_then_load_roundtrip() {
        let key = Vault::generate_key().unwrap();
        let vault = Vault::new(&key).unwrap();

        let kp = Keypair::new();
        let original_pubkey = kp.pubkey();

        let secrets = VaultSecrets {
            webhook_secret: "test-webhook-secret".to_string(),
            webhook_secret_previous: None,
            wallet_private_key: Some(hex::encode(kp.to_bytes())),
            rpc_api_key: None,
            fallback_rpc_api_key: None,
        };

        let tmp = NamedTempFile::new().unwrap();
        vault.save_secrets(&secrets, tmp.path()).unwrap();

        let reloaded = vault.load_secrets(tmp.path()).unwrap();
        let kp2 = load_wallet_keypair(&reloaded).unwrap();
        assert_eq!(kp2.pubkey(), original_pubkey);
    }

    /// All three input formats should produce the same 64-byte buffer.
    #[test]
    fn all_formats_agree() {
        let kp = Keypair::new();
        let bytes = kp.to_bytes();

        let hex_str = hex::encode(bytes);
        let b58 = bs58::encode(bytes).into_string();
        let json = serde_json::to_string(&bytes.to_vec()).unwrap();

        assert_eq!(*normalize_to_64_bytes(&hex_str).unwrap(), bytes);
        assert_eq!(*normalize_to_64_bytes(&b58).unwrap(), bytes);
        assert_eq!(*normalize_to_64_bytes(&json).unwrap(), bytes);
    }

    /// `print_plan` must never panic on `None` fields.
    #[test]
    fn print_plan_handles_none() {
        let secrets = fresh_secrets("wh");
        // Should not panic; output goes to stdout and is discarded by the test.
        print_plan(std::path::Path::new("/tmp/x"), &secrets, "pubkey", true);
    }

    // ==========================================================================
    // ADDITIONAL COVERAGE
    // ==========================================================================

    #[test]
    fn print_plan_all_field_states() {
        let secrets = VaultSecrets {
            webhook_secret: "wh-secret".to_string(),
            webhook_secret_previous: Some("old-secret".to_string()),
            wallet_private_key: Some("ab12cd".to_string()),
            rpc_api_key: Some("rpc-key".to_string()),
            fallback_rpc_api_key: Some("fb-key".to_string()),
        };
        // Dry-run header + full fields.
        print_plan(
            std::path::Path::new("/tmp/plan-a"),
            &secrets,
            "pubkey1",
            true,
        );
        // Real-run header.
        print_plan(
            std::path::Path::new("/tmp/plan-b"),
            &secrets,
            "pubkey2",
            false,
        );
    }

    #[test]
    fn fresh_secrets_builds_empty_bundle() {
        let secrets = fresh_secrets("wh");
        assert_eq!(secrets.webhook_secret, "wh");
        assert!(secrets.webhook_secret_previous.is_none());
        assert!(secrets.wallet_private_key.is_none());
        assert!(secrets.rpc_api_key.is_none());
        assert!(secrets.fallback_rpc_api_key.is_none());
    }

    #[test]
    fn read_keypair_input_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kp.json");
        std::fs::write(&path, b"[1,2,3]").unwrap();
        let contents = read_keypair_input(Some(&path)).unwrap();
        assert_eq!(contents, "[1,2,3]");
    }

    #[test]
    fn read_keypair_input_missing_file_errors() {
        let result = read_keypair_input(Some(std::path::Path::new(
            "/nonexistent/definitely-missing-kp.json",
        )));
        assert!(result.is_err());
    }

    #[test]
    fn preflight_roundtrip_ok_and_bad_keypair() {
        let key = Vault::generate_key().unwrap();
        let vault = Vault::new(&key).unwrap();
        let kp = Keypair::new();
        let secrets = VaultSecrets {
            webhook_secret: "wh".to_string(),
            webhook_secret_previous: None,
            wallet_private_key: Some(hex::encode(kp.to_bytes())),
            rpc_api_key: None,
            fallback_rpc_api_key: None,
        };
        preflight_roundtrip(&vault, &secrets).unwrap();

        // A garbage keypair must fail the preflight (no disk touched).
        let bad = VaultSecrets {
            webhook_secret: secrets.webhook_secret.clone(),
            webhook_secret_previous: None,
            wallet_private_key: Some("not-a-keypair".to_string()),
            rpc_api_key: None,
            fallback_rpc_api_key: None,
        };
        assert!(preflight_roundtrip(&vault, &bad).is_err());
    }

    #[test]
    fn roundtrip_validate_success_and_mismatch() {
        let key = Vault::generate_key().unwrap();
        let vault = Vault::new(&key).unwrap();
        let kp = Keypair::new();
        let secrets = VaultSecrets {
            webhook_secret: "wh".to_string(),
            webhook_secret_previous: None,
            wallet_private_key: Some(hex::encode(kp.to_bytes())),
            rpc_api_key: None,
            fallback_rpc_api_key: None,
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        vault.save_secrets(&secrets, tmp.path()).unwrap();

        let expected = bs58::encode(kp.pubkey().as_ref()).into_string();
        roundtrip_validate(&vault, tmp.path(), &expected).unwrap();

        assert!(roundtrip_validate(&vault, tmp.path(), "wrong-pubkey").is_err());
        // Corrupt file → re-open fails.
        std::fs::write(tmp.path(), b"garbage").unwrap();
        assert!(roundtrip_validate(&vault, tmp.path(), &expected).is_err());
    }

    #[test]
    fn copy_file_0600_creates_copy() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::write(&src, b"secret-bytes").unwrap();
        copy_file_0600(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"secret-bytes");
        // Overwrite path also works.
        copy_file_0600(&src, &dst).unwrap();
    }

    #[test]
    fn restore_backup_no_prior_removes_target() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("secrets.enc.bak");
        let target = dir.path().join("secrets.enc");
        std::fs::write(&target, b"corrupt").unwrap();
        std::fs::write(&backup, b"old").unwrap();
        let outcome = restore_backup(&backup, &target, false);
        assert!(matches!(outcome, RestoreOutcome::NoPrior));
        assert!(!target.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn restore_backup_renames_when_prior_exists() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("secrets.enc.bak");
        let target = dir.path().join("secrets.enc");
        std::fs::write(&backup, b"good-old-vault").unwrap();
        std::fs::write(&target, b"corrupt-new").unwrap();
        let outcome = restore_backup(&backup, &target, true);
        assert!(matches!(outcome, RestoreOutcome::Restored));
        assert_eq!(std::fs::read(&target).unwrap(), b"good-old-vault");
        assert!(!backup.exists(), "backup consumed by rename");
    }

    #[test]
    fn restore_backup_copy_fallback_when_rename_fails() {
        // Make rename fail by turning the target into a non-empty directory
        // (rename over a directory fails on POSIX).
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("secrets.enc.bak");
        let target = dir.path().join("secrets.enc");
        std::fs::write(&backup, b"good-old-vault").unwrap();
        std::fs::create_dir(&target).unwrap();
        let outcome = restore_backup(&backup, &target, true);
        // Either the copy fallback restored (directory copy fails → FailedBackupIntact)
        // — assert we never panic and return one of the two handled outcomes.
        assert!(
            matches!(
                outcome,
                RestoreOutcome::Restored | RestoreOutcome::FailedBackupIntact
            ),
            "must not panic on rename failure"
        );
    }

    #[test]
    fn restore_backup_both_fail_keeps_backup_intact() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("secrets.enc.bak");
        let target = dir.path().join("secrets.enc");
        // Back up path is a DIRECTORY → rename (dir over file) fails and the
        // copy fallback's read fails → FailedBackupIntact.
        std::fs::create_dir(&backup).unwrap();
        std::fs::write(&target, b"corrupt").unwrap();
        let outcome = restore_backup(&backup, &target, true);
        assert!(matches!(outcome, RestoreOutcome::FailedBackupIntact));
    }

    // ==========================================================================
    // RUN() — the full import pipeline (env + temp paths)
    // ==========================================================================

    /// Serialize env-touching tests: cargo runs tests in parallel threads and
    /// process-wide env vars would otherwise race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_env() {
        for var in [
            "CHIMERA_VAULT_KEY",
            "CHIMERA_VAULT_PATH",
            "CHIMERA_SECURITY__WEBHOOK_SECRET",
            "CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS",
            "HELIUS_API_KEY",
            "CHIMERA_RPC__FALLBACK_API_KEY",
        ] {
            std::env::remove_var(var);
        }
    }

    fn cli_with_keypair_file(dir: &tempfile::TempDir, extra: &[&str]) -> Cli {
        let path = dir.path().join("kp.json");
        let kp = Keypair::new();
        std::fs::write(&path, serde_json::to_string(&kp.to_bytes().to_vec()).unwrap()).unwrap();
        let mut args = vec!["import_keypair", "--keypair-file", path.to_str().unwrap()];
        args.extend_from_slice(extra);
        Cli::parse_from(args)
    }

    fn env_key() -> String {
        Vault::generate_key().unwrap()
    }

    #[test]
    fn run_fails_without_vault_key_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::remove_var("CHIMERA_VAULT_KEY");
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "x");
        let dir = tempfile::tempdir().unwrap();
        let err = run(cli_with_keypair_file(&dir, &[])).unwrap_err();
        assert!(err.to_string().contains("openssl rand"), "{err}");
    }

    #[test]
    fn run_fails_without_webhook_secret_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::remove_var("CHIMERA_SECURITY__WEBHOOK_SECRET");
        let dir = tempfile::tempdir().unwrap();
        let err = run(cli_with_keypair_file(&dir, &[])).unwrap_err();
        assert!(err
            .to_string()
            .contains("CHIMERA_SECURITY__WEBHOOK_SECRET"), "{err}");
    }

    #[test]
    fn run_rejects_invalid_keypair_bytes() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not a keypair").unwrap();
        let err = run(Cli::parse_from([
            "import_keypair",
            "--keypair-file",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("Keypair validation failed"), "{err}");
    }

    #[test]
    fn run_rejects_invalid_ed25519_bytes() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad64.json");
        // 64 bytes of garbage: valid length, invalid ed25519.
        std::fs::write(&path, serde_json::to_string(&vec![7u8; 64]).unwrap()).unwrap();
        let err = run(Cli::parse_from([
            "import_keypair",
            "--keypair-file",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("not a valid Ed25519 keypair"), "{err}");
    }

    #[test]
    fn run_rejects_inconsistent_secret_and_pubkey() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mismatch.json");
        // 32 valid secret bytes + 32 unrelated pubkey bytes.
        let secret = Keypair::new().to_bytes()[..32].to_vec();
        let other = Keypair::new().pubkey().to_bytes();
        let mut mixed = secret;
        mixed.extend_from_slice(&other);
        std::fs::write(&path, serde_json::to_string(&mixed).unwrap()).unwrap();
        // This solana-sdk version's Keypair::try_from already verifies that
        // the public half derives from the secret half, so the inconsistency
        // is rejected there (the probe-message branch in `run` is dead for
        // this SDK — kept as defense-in-depth for older SDKs).
        let err = run(Cli::parse_from([
            "import_keypair",
            "--keypair-file",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(
            err.to_string().contains("not a valid Ed25519 keypair"),
            "{err}"
        );
    }

    #[test]
    fn run_dry_run_writes_nothing() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.enc");
        run(cli_with_keypair_file(
            &dir,
            &["--vault-path", vault_path.to_str().unwrap(), "--dry-run"],
        ))
        .unwrap();
        assert!(!vault_path.exists(), "dry-run must not write");
    }

    #[test]
    fn run_fresh_vault_end_to_end() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let key = env_key();
        std::env::set_var("CHIMERA_VAULT_KEY", &key);
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        std::env::set_var("HELIUS_API_KEY", "helius-key-123");
        std::env::set_var("CHIMERA_RPC__FALLBACK_API_KEY", "fb-key-456");
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.enc");
        let kp_path = dir.path().join("kp.json");
        let kp = Keypair::new();
        std::fs::write(&kp_path, serde_json::to_string(&kp.to_bytes().to_vec()).unwrap()).unwrap();
        let expected_pubkey = bs58::encode(kp.pubkey().as_ref()).into_string();
        run(Cli::parse_from([
            "import_keypair",
            "--keypair-file",
            kp_path.to_str().unwrap(),
            "--vault-path",
            vault_path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(vault_path.exists());
        // No prior vault → no backup left behind (never created).
        assert!(!dir.path().join("secrets.enc.bak").exists());

        // Round-trip: re-open with the same key and confirm pubkey + secrets.
        let vault = Vault::new(&key).unwrap();
        let secrets = vault.load_secrets(&vault_path).unwrap();
        assert_eq!(secrets.webhook_secret, "0123456789abcdef0123456789abcdef");
        assert_eq!(secrets.rpc_api_key.as_deref(), Some("helius-key-123"));
        assert_eq!(secrets.fallback_rpc_api_key.as_deref(), Some("fb-key-456"));
        let kp = load_wallet_keypair(&secrets).unwrap();
        assert_eq!(bs58::encode(kp.pubkey().as_ref()).into_string(), expected_pubkey);
    }

    #[test]
    fn run_rotates_existing_vault_and_cleans_backup() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let key = env_key();
        std::env::set_var("CHIMERA_VAULT_KEY", &key);
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "new-secret-0123456789abcdef0123");
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.enc");

        // Prior vault with a DIFFERENT webhook secret + old keypair.
        let vault = Vault::new(&key).unwrap();
        let old_kp = Keypair::new();
        vault
            .save_secrets(
                &VaultSecrets {
                    webhook_secret: "old-secret-0123456789abcdef0123456789".to_string(),
                    webhook_secret_previous: None,
                    wallet_private_key: Some(hex::encode(old_kp.to_bytes())),
                    rpc_api_key: None,
                    fallback_rpc_api_key: None,
                },
                &vault_path,
            )
            .unwrap();

        run(cli_with_keypair_file(
            &dir,
            &["--vault-path", vault_path.to_str().unwrap()],
        ))
        .unwrap();

        // Rotated: new secret moved to previous, new keypair in place, backup removed.
        let secrets = vault.load_secrets(&vault_path).unwrap();
        assert_eq!(secrets.webhook_secret, "new-secret-0123456789abcdef0123");
        assert_eq!(
            secrets.webhook_secret_previous.as_deref(),
            Some("old-secret-0123456789abcdef0123456789")
        );
        assert!(!dir.path().join("secrets.enc.bak").exists(), "backup must be removed on success");
    }

    #[test]
    fn run_honors_previous_secret_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "same-secret-0123456789abcdef0123");
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS", "prev-secret-from-env");
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.enc");
        run(cli_with_keypair_file(
            &dir,
            &["--vault-path", vault_path.to_str().unwrap()],
        ))
        .unwrap();
        let vault = Vault::new(&std::env::var("CHIMERA_VAULT_KEY").unwrap()).unwrap();
        let secrets = vault.load_secrets(&vault_path).unwrap();
        assert_eq!(
            secrets.webhook_secret_previous.as_deref(),
            Some("prev-secret-from-env")
        );
    }

    #[test]
    fn run_vault_path_from_env_and_existing_undecryptable_vault() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.enc");
        std::env::set_var("CHIMERA_VAULT_PATH", vault_path.to_str().unwrap());
        run(cli_with_keypair_file(&dir, &[])).unwrap();
        assert!(vault_path.exists(), "CHIMERA_VAULT_PATH must be honored");
        std::env::remove_var("CHIMERA_VAULT_PATH");

        // Now a vault written under a DIFFERENT key → decrypt failure bail.
        let other_key = env_key();
        let other = Vault::new(&other_key).unwrap();
        other
            .save_secrets(
                &VaultSecrets {
                    webhook_secret: "x".to_string(),
                    webhook_secret_previous: None,
                    wallet_private_key: None,
                    rpc_api_key: None,
                    fallback_rpc_api_key: None,
                },
                &vault_path,
            )
            .unwrap();
        let err = run(cli_with_keypair_file(
            &dir,
            &["--vault-path", vault_path.to_str().unwrap()],
        ))
        .unwrap_err();
        assert!(err.to_string().contains("could not be decrypted"), "{err}");
    }

    #[test]
    fn run_save_failure_restores_nothing_and_errors() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        std::env::set_var("CHIMERA_VAULT_KEY", env_key());
        std::env::set_var("CHIMERA_SECURITY__WEBHOOK_SECRET", "0123456789abcdef0123456789abcdef");
        let dir = tempfile::tempdir().unwrap();
        // Read-only parent: exists() is false for the target file (fresh
        // vault), but save_secrets' O_EXCL tmp create fails with EACCES.
        let ro_dir = dir.path().join("ro");
        std::fs::create_dir(&ro_dir).unwrap();
        let vault_path = ro_dir.join("secrets.enc");
        #[cfg(unix)]
        {
            std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        let err = run(cli_with_keypair_file(
            &dir,
            &["--vault-path", vault_path.to_str().unwrap()],
        ))
        .unwrap_err();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(err.to_string().contains("Failed to write vault"), "{err}");
        assert!(!vault_path.exists());
    }

    #[test]
    fn cli_parse_all_flags() {
        let cli = Cli::parse_from([
            "import_keypair",
            "--keypair-file",
            "/tmp/kp",
            "--vault-path",
            "/tmp/vault.enc",
            "--dry-run",
        ]);
        assert_eq!(
            cli.keypair_file.as_deref(),
            Some(std::path::Path::new("/tmp/kp"))
        );
        assert_eq!(
            cli.vault_path.as_deref(),
            Some(std::path::Path::new("/tmp/vault.enc"))
        );
        assert!(cli.dry_run);
    }
}
