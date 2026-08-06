use anyhow::Result;
use rand::Rng;
use std::fmt::Write as _;

/// Generate a cryptographically strong JWT secret
pub fn generate_jwt_secret() -> Result<String> {
    let mut rng = rand::rng();
    let mut secret = String::with_capacity(64);
    for _ in 0..64 {
        write!(secret, "{:x}", rng.random_range(0..16))?;
    }

    // Verify it meets requirements
    if secret.len() != 64 || !secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!("Generated secret failed validation"));
    }

    Ok(secret)
}

pub fn main() -> Result<()> {
    let secret = generate_jwt_secret()?;
    // writeln! so a closed stdout (e.g. `| head`) fails cleanly instead of
    // panicking with a trace.
    use std::io::Write as _;
    writeln!(std::io::stdout().lock(), "{}", secret)?;
    Ok(())
}
