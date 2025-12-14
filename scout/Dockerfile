# Dockerfile for Chimera Scout (Python)
FROM python:3.11-slim

# Install system dependencies
RUN apt-get update && apt-get install -y \
    gcc \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy requirements first for better caching
COPY scout/requirements.txt .

# Install Python dependencies
RUN pip install --no-cache-dir -r requirements.txt

# Copy shared database schema from project root
# This ensures the schema file is available for RosterWriter
# The schema is the source of truth used by both Rust and Python
COPY database/schema/wallets.sql /app/database/schema/wallets.sql

# Copy scout code
COPY scout/ .

# Create app user
RUN useradd -m -u 1000 chimera && \
    chown -R chimera:chimera /app

# Switch to non-root user
USER chimera

# Default command (can be overridden in docker-compose)
CMD ["python", "main.py"]
