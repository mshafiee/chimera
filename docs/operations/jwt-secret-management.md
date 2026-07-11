# JWT Secret Management

## Generating a Strong JWT Secret

Use the provided tool to generate a cryptographically strong JWT secret:

```bash
cd operator
cargo run --bin generate_jwt_secret
```

This generates a 64-character hexadecimal string (256 bits of entropy).

## Setting the Secret

Set the generated secret in your environment:

```bash
export JWT_SECRET=<generated-secret>
```

## Requirements

The JWT secret must:
- Be at least 64 characters (256 bits of entropy)
- Use only hexadecimal characters (0-9, a-f, A-F)
- Not match common weak patterns (all zeros, all ones, repeated patterns)

## Migration for Existing Deployments

If your existing secret doesn't meet these requirements:

1. Generate a new secret using `cargo run --bin generate_jwt_secret`
2. Update your environment variable
3. Restart the operator
4. All existing JWT tokens will become invalid (users will need to re-authenticate)

## Development Mode

In non-production mode (when `CHIMERA_ENV` is not set to "production"), secret strength validation is skipped for convenience and a weak default is generated. Never use this in production.