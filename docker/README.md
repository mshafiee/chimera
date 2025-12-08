# Docker Configuration for Chimera

This directory contains Docker Compose configuration files and helper scripts for running Chimera in different environments.

## Quick Start

### Using the Helper Script

```bash
# Start devnet environment
./docker-compose.sh start devnet

# View logs
./docker-compose.sh logs devnet -f

# Access operator shell
./docker-compose.sh shell devnet operator

# Stop services
./docker-compose.sh stop devnet
```

### Using Make (from docker/ directory)

```bash
# Devnet
make devnet-start
make devnet-logs
make devnet-shell

# Paper Trading
make paper-start
make paper-logs

# Production
make prod-start
make prod-logs
```

## Files

- `env.devnet` - Devnet environment configuration template
- `env.mainnet-paper` - Mainnet paper trading configuration template
- `env.mainnet-prod` - Mainnet production configuration template
- `docker-compose.sh` - Helper script for managing environments
- `init-db.sh` - Database initialization script
- `Makefile` - Convenient make targets

## Configuration

1. Copy the appropriate environment file:
   ```bash
   cp docker/env.devnet docker/env.devnet.local
   ```

2. Edit the local file with your settings:
   ```bash
   nano docker/env.devnet.local
   ```

3. The docker-compose.yml will automatically use the profile-specific environment file.

## See Also

- [Main Docker Compose README](../docker-compose.README.md) - Complete documentation
- [Main README](../README.md) - Project overview
