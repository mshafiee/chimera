#![allow(warnings)]

use anyhow::Result;
use rand::Rng;

/// Generate a cryptographically strong JWT secret
pub fn generate_jwt_secret() -> Result<String> {
    let mut rng = rand::thread_rng();
    let secret: String = (0..64)
        .map(|_| format!("{:x}", rng.gen_range(0..16)))
        .collect();

    // Verify it meets requirements
    if secret.len() != 64 || !secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!("Generated secret failed validation"));
    }

    Ok(secret)
}

fn main() -> Result<()> {
    let secret = generate_jwt_secret()?;
    println!("{}", secret);
    Ok(())
}