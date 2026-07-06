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

# Copy Cargo files first for better caching
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY migrations_postgres ./migrations_postgres

# Build release binary (online mode for sqlx, as we don't have prepared data)
RUN cargo build --release

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
