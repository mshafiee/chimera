# Multi-stage build for Chimera Operator (Rust)
# Using Ubuntu 22.04 for GLIBC compatibility
FROM ubuntu:22.04 as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Set working directory
WORKDIR /build

# Allow stamping the real git commit hash into the binary. Passed via
# --build-arg GIT_HASH=$(git rev-parse HEAD) on the host; defaults to
# "unknown" when not provided (build.rs resolves it / falls back).
ARG GIT_HASH=unknown
ENV GIT_HASH=${GIT_HASH}

# ── Cargo workspace layout (2026-08-07) ────────────────────────────────────
# The operator is now a member of the workspace rooted at the repo root:
#   Cargo.toml           workspace manifest ([workspace.dependencies])
#   core/                domain-pure crate (chimera_core)
#   operator/            this crate (chimera_operator)
# Build context must be the repo root (see docker-compose.yml).

# Copy workspace manifests first for better caching
COPY Cargo.toml Cargo.lock ./
COPY api/Cargo.toml ./api/Cargo.toml
COPY core/Cargo.toml ./core/Cargo.toml
COPY infra/Cargo.toml ./infra/Cargo.toml
COPY operator/Cargo.toml ./operator/Cargo.toml
COPY operator/build.rs ./operator/build.rs

# Copy sources
COPY api/src ./api/src
COPY core/src ./core/src
COPY infra/src ./infra/src
COPY infra/migrations_postgres ./infra/migrations_postgres
COPY operator/src ./operator/src

# Build release binary (online mode for sqlx, as we don't have prepared data)
# The chimera_operator binary now lives in the api crate (kept name for
# deploy compatibility); operator's CLI bins (import_keypair) still build from operator.
RUN cargo build --release -p chimera_api -p chimera_operator --bins

# Runtime stage - same base image for GLIBC compatibility
FROM ubuntu:22.04

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -m -u 1000 chimera

# Set working directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/chimera_operator /app/chimera_operator
# Ship the import_keypair CLI alongside the operator so the runbook's
# `docker compose run ... /app/import_keypair` procedure works.
COPY --from=builder /build/target/release/import_keypair /app/import_keypair
# One-shot Dune bootstrap (wallet PnL history + roster seed). Runs manually
# via `docker compose run --rm bootstrap_dune bootstrap_dune --apply --roster`.
COPY --from=builder /build/target/release/bootstrap_dune /app/bootstrap_dune
# Offline exit-rule replay harness (analyzes reconstructed price paths against
# the real exit rules; used by scout copy_backtest CLI --binary). One-shot.
COPY --from=builder /build/target/release/replay_exit /app/replay_exit

# Create directories (config files are mounted as volumes in docker-compose)
RUN mkdir -p /app/config /app/data && chown -R chimera:chimera /app

# Switch to non-root user
USER chimera

# Expose port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/api/v1/health || exit 1

# Run the operator
CMD ["/app/chimera_operator"]
