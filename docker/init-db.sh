#!/bin/bash
# Initialize database for Docker Compose setup

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DATA_DIR="${1:-$PROJECT_ROOT/data}"
SCHEMA_FILE="${2:-$PROJECT_ROOT/database/schema.sql}"

echo "Initializing Chimera database..."

# Create data directory if it doesn't exist
mkdir -p "$DATA_DIR"

# Check if database already exists
if [ -f "$DATA_DIR/chimera.db" ]; then
    echo "Database already exists at $DATA_DIR/chimera.db"
    if [ -t 0 ]; then
        read -p "Do you want to recreate it? (y/N): " -n 1 -r
        echo
    else
        REPLY="n"  # Non-interactive: keep the existing database
    fi
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Skipping database initialization."
        exit 0
    fi
    rm -f "$DATA_DIR/chimera.db" "$DATA_DIR/chimera.db-shm" "$DATA_DIR/chimera.db-wal" "$DATA_DIR/chimera.db-journal"
fi

# Initialize database
if [ -f "$SCHEMA_FILE" ]; then
    sqlite3 "$DATA_DIR/chimera.db" < "$SCHEMA_FILE"
    echo "✓ Database initialized successfully at $DATA_DIR/chimera.db"
else
    echo "✗ Error: Schema file not found at $SCHEMA_FILE"
    exit 1
fi

# Set permissions
chmod 644 "$DATA_DIR/chimera.db"

echo "Database initialization complete!"
