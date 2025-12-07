# Project Chimera

<div align="center">

**High-Frequency, Fault-Tolerant Copy-Trading Platform for Solana**

*Barbell Strategy Execution • Sub-5ms Internal Latency • Institutional-Grade Resilience*

[![Version](https://img.shields.io/badge/version-v7.1_(Engineering_Freeze)-blue)](https://github.com/mshafiee/chimera)
[![Status](https://img.shields.io/badge/status-production--ready-success)](https://github.com/mshafiee/chimera)
[![License](https://img.shields.io/badge/license-MIT-purple)](LICENSE)
[![Stack](https://img.shields.io/badge/stack-Rust_%7C_Python_%7C_TypeScript-orange)](https://github.com/mshafiee/chimera)

</div>

---

## ⚠️ Critical Infrastructure Requirements

**Chimera is a High-Frequency Trading (HFT) system. Physical server location is critical for performance.**

- **Required Location:** Servers must be in **Ashburn, VA (US-East)** or **Amsterdam**
- **Do NOT deploy from:** Hetzner Falkenstein, Helsinki, or other high-latency locations
- **Latency Requirement:** RPC endpoint latency must be **< 50ms** (verify with `ping -c 10 <helius-endpoint>`)
- **Recommended Providers:**
  - **Latitude.sh** (formerly Maxihost): Bare metal in Ashburn/NY
  - **Cherry Servers:** Bare metal in US-East
  - **Hetzner:** Ashburn DC only (if available)

> **Why this matters:** A 100ms roundtrip latency to US-East RPCs defeats the purpose of the "<5ms internal latency" optimization and will cause failed trades due to blockhash expiration.

---

## 📖 Overview

**Project Chimera** is an automated copy-trading platform for Solana that executes trades based on signals from tracked wallets. Unlike simple copy-trading bots, Chimera implements a sophisticated **Barbell Strategy** that balances capital preservation with asymmetric upside potential.

### Key Differentiators

- **Sub-5ms Internal Latency:** Rust-based hot path for ultra-fast execution
- **Fault-Tolerant Architecture:** Automatic RPC failover, circuit breakers, and self-healing mechanisms
- **Intelligent Wallet Selection:** Wallet Quality Score (WQS) v2 with pre-promotion backtesting
- **Token Safety:** Automated honeypot detection, liquidity checks, and freeze/mint authority validation
- **Production-Ready Operations:** Comprehensive monitoring, alerting, runbooks, and compliance features

---

## 🛡️ The Barbell Strategy

Chimera employs a **Barbell Strategy** that balances two complementary approaches:

### 🛡️ The Shield (Capital Preservation)
- **Focus:** Low-risk, high-consistency trades
- **Behavior:** Copies proven "Alpha Hunters" with strict stop-losses and liquidity checks
- **Goal:** Generate consistent profits to cover operational costs and protect principal
- **Liquidity Threshold:** Minimum $10,000 USD per token

### ⚔️ The Spear (Asymmetric Upside)
- **Focus:** High-risk, high-reward opportunities
- **Behavior:** Activates only on high-conviction signals using Jito Bundles for guaranteed block inclusion
- **Goal:** Hunt for 50x-100x outlier opportunities
- **Liquidity Threshold:** Minimum $5,000 USD per token
- **Safety:** Automatically disabled if RPC instability or consecutive losses detected

---

## 🏗️ System Architecture

Chimera uses a **Hot/Cold Architecture** to optimize for both speed and intelligence:

### The Hot Path (Rust Operator)
The Operator handles real-time trade execution with sub-millisecond latency:

```
External Signal Provider
    ↓
Webhook Endpoint (/api/v1/webhook)
    ├─ HMAC Signature Verification
    ├─ Rate Limiting (100 req/s)
    └─ Token Safety Checks
    ↓
Priority Queue (EXIT > SHIELD > SPEAR)
    ├─ Load Shedding (drops SPEAR if queue > 80%)
    └─ Circuit Breaker Check
    ↓
Execution Engine
    ├─ Jito Bundle Submission (Primary)
    ├─ Standard TPU (Fallback)
    └─ Transaction Confirmation
    ↓
Position Tracking & Notifications
```

**Key Components:**
- **Ingress:** DDoS protection, HMAC verification, replay attack prevention
- **Executor:** Smart routing between Jito Bundles and standard TPU
- **Recovery Manager:** Automatic stuck-state recovery for positions
- **Circuit Breaker:** Risk management with automatic trading halts
- **Token Parser:** Fast/slow path token safety validation

### The Cold Path (Python Scout)
The Scout runs periodically (via cron) to analyze wallet performance:

```
Daily Cron Job
    ↓
Wallet Analyzer
    ├─ Fetch transaction history
    ├─ Calculate Wallet Quality Score (WQS)
    └─ Run pre-promotion backtests
    ↓
Pre-Promotion Validator
    ├─ Historical liquidity checks
    ├─ Simulated PnL validation
    └─ Risk assessment
    ↓
Roster Writer
    └─ Atomic write to roster_new.db
    ↓
Operator Merge (via SIGHUP or API)
```

**Key Components:**
- **Analyzer:** Fetches wallet transaction history from Solana
- **WQS Calculator:** Computes wallet quality scores with temporal consistency penalties
- **Backtester:** Pre-promotion trade simulation with historical liquidity validation
- **DB Writer:** Atomic writes to prevent database corruption

### Web Dashboard (TypeScript/React)
Real-time monitoring and management interface with:
- Real-time position tracking via WebSocket
- Wallet management and promotion
- Trade history and export (CSV/PDF)
- Configuration management
- Performance metrics visualization
- Incident log and dead letter queue

---

## ✨ Key Features

### 🔒 Security & Safety
- **HMAC Authentication:** Replay attack prevention with timestamp validation
- **Token Safety Checks:** Automated detection of honeypots, freeze authority, and mint authority
- **Circuit Breakers:** Automatic trading halts on loss thresholds
- **Idempotency:** Deterministic UUID generation prevents double-execution
- **Secret Rotation:** Automated secret rotation with grace period support
- **Encrypted Vault:** AES-256 encrypted storage for private keys

### ⚡ Resilience
- **Priority Queuing:** Load shedding drops low-priority signals during high load
- **RPC Failover:** Automatic switch from Helius to QuickNode/Triton on failures
- **Self-Healing:** Database write-lock mitigation via WAL mode and SQL-level merges
- **Stuck State Recovery:** Automatic recovery for positions stuck in EXITING state
- **Graceful Degradation:** Dead letter queue for failed operations

### 📊 Intelligence
- **Wallet Quality Score (WQS) v2:** Advanced scoring with temporal consistency penalties
- **Pre-Promotion Backtesting:** Historical trade simulation before wallet activation
- **Historical Liquidity Validation:** Ensures trades would have been profitable with past liquidity
- **Dynamic Jito Tips:** Percentile-based tip calculation for optimal bundle inclusion

### 🎯 Operations & Compliance
- **Trade Reconciliation:** Daily audit comparing database state vs. on-chain state
- **Audit Logs:** Immutable logs for configuration changes and failed operations
- **Prometheus Metrics:** Comprehensive observability with Grafana dashboards
- **Alertmanager Integration:** Automated alerting for critical events
- **Incident Runbooks:** Comprehensive documentation for common failure modes
- **Export Functionality:** CSV/PDF export for audit and tax reporting

---

## 🛠️ Tech Stack

| Component | Technology |
|-----------|-----------|
| **Core Engine** | Rust (Tokio, Axum, Tower-Governor) |
| **Intelligence** | Python 3.11+ (Pandas, NumPy, Hypothesis) |
| **Database** | SQLite (WAL Mode, SQLx) |
| **Blockchain** | Solana SDK, Jito Block Engine, Jupiter Swap API |
| **Frontend** | TypeScript, React, Vite, Tailwind CSS |
| **Observability** | Prometheus, Grafana, Alertmanager |
| **Testing** | Rust tests, Pytest, Playwright (E2E), k6 (load) |

---

## 🚀 Getting Started

### Prerequisites

- **Rust 1.75+** and Cargo
- **Python 3.11+** with pip
- **Node.js 18+** and npm
- **SQLite 3.x**
- **Helius API Key** (Developer or Pro tier recommended)
- **Server in US-East (Ashburn, VA)** or Amsterdam

### Installation

1. **Clone the repository:**
   ```bash
   git clone https://github.com/mshafiee/chimera.git
   cd chimera
   ```

2. **Build the Operator:**
   ```bash
   cd operator
   cargo build --release
   ```

3. **Install Scout dependencies:**
   ```bash
   cd ../scout
   pip install -r requirements.txt
   ```

4. **Install Web dependencies:**
   ```bash
   cd ../web
   npm install
   ```

5. **Initialize the database:**
   ```bash
   cd ..
   mkdir -p data
   sqlite3 data/chimera.db < database/schema.sql
   ```

### Configuration

1. **Create environment file:**
   ```bash
   cd operator
   cp config/.env.example .env
   ```

2. **Edit `.env` with your configuration:**
   ```bash
   # Required: Generate with `openssl rand -hex 32`
   CHIMERA_SECURITY__WEBHOOK_SECRET=your-64-char-hex-secret

   # Required: Your Helius RPC endpoint
   CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_API_KEY

   # Optional: Fallback RPC
   CHIMERA_RPC__FALLBACK_URL=https://your-quicknode-endpoint.com

   # Optional: Telegram notifications
   TELEGRAM_BOT_TOKEN=your-bot-token
   TELEGRAM_CHAT_ID=your-chat-id

   # Optional: Discord webhook
   DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...

   # Development mode (skips validation)
   CHIMERA_DEV_MODE=1
   ```

3. **Configure `config/config.yaml`:**
   - Adjust circuit breaker thresholds
   - Set strategy allocation (Shield/Spear percentages)
   - Configure Jito tip settings
   - Set token safety thresholds

### Running the System

**Terminal 1: Start the Operator**
```bash
cd operator
cargo run --release
# Or for development with debug logging:
RUST_LOG=chimera_operator=debug cargo run
```

The Operator will start on `http://0.0.0.0:8080` by default.

**Terminal 2: Run the Scout (Intelligence Layer)**
```bash
cd scout
python main.py
# Or run with options:
python main.py --verbose --min-wqs-active 70
```

**Terminal 3: Start the Web Dashboard (Development)**
```bash
cd web
npm run dev
```

The dashboard will be available at `http://localhost:5173` (or your configured port).

### Testing the Webhook

Send a test signal with valid HMAC signature:

```bash
# Generate signature
TIMESTAMP=$(date +%s)
PAYLOAD='{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"}'
SECRET="your-webhook-secret"
SIGNATURE=$(echo -n "${TIMESTAMP}${PAYLOAD}" | openssl dgst -sha256 -hmac "$SECRET" | cut -d' ' -f2)

# Send request
curl -X POST http://localhost:8080/api/v1/webhook \
  -H "Content-Type: application/json" \
  -H "X-Signature: ${SIGNATURE}" \
  -H "X-Timestamp: ${TIMESTAMP}" \
  -d "${PAYLOAD}"
```

### Health Check

```bash
curl http://localhost:8080/api/v1/health
```

---

## 📚 Documentation

Comprehensive documentation is available in the `docs/` directory:

- **[Product Design Document](docs/pdd.md)** - Complete system specification
- **[Architecture Documentation](docs/architecture.md)** - System design and components
- **[API Documentation](docs/api.md)** - REST API reference
- **[Pre-Deployment Checklist](docs/pre-deployment-checklist.md)** - Deployment verification steps
- **[Security Audit Checklist](docs/security-audit-checklist.md)** - Security best practices
- **[Runbooks](ops/runbooks/)** - Incident response procedures

---

## 🧪 Testing

### Run All Tests
```bash
make test
```

### Run Specific Test Suites
```bash
# Unit tests
make test-operator
make test-scout

# Integration tests
make test-integration

# Load tests (requires k6)
make test-load

# Chaos tests
make test-chaos

# E2E tests (requires Playwright)
cd web && npm run test:e2e
```

### Test Coverage
- **Unit Tests:** WQS calculation, token parser, circuit breaker, state machine
- **Integration Tests:** API endpoints, authentication, webhook flow, database operations
- **Chaos Tests:** RPC fallback, mid-trade failures, database locks
- **Load Tests:** Webhook flood (100 req/sec), queue saturation
- **E2E Tests:** Dashboard, wallet promotion, configuration, trade ledger

See [Test Coverage Summary](docs/test-coverage-summary.md) for details.

---

## 🔧 Development

### Development Workflow

1. **Start development server:**
   ```bash
   make dev-operator
   ```

2. **Run linters:**
   ```bash
   make lint
   ```

3. **Format code:**
   ```bash
   make fmt
   ```

4. **Run security audits:**
   ```bash
   make audit
   ```

### Project Structure

```
chimera/
├── operator/          # Rust Operator (hot path)
│   ├── src/
│   │   ├── engine/    # Execution engine
│   │   ├── handlers/  # API handlers
│   │   ├── token/     # Token safety checks
│   │   └── ...
│   └── tests/         # Rust tests
├── scout/             # Python Scout (cold path)
│   ├── core/          # WQS, backtester, analyzer
│   └── tests/         # Python tests
├── web/               # TypeScript/React dashboard
│   ├── src/
│   └── tests/
├── ops/               # Operational scripts
│   ├── runbooks/      # Incident runbooks
│   └── prometheus/    # Monitoring config
├── database/          # SQLite schema
└── docs/              # Documentation
```

---

## 🚢 Deployment

### Pre-Deployment Verification

**Critical:** Run preflight checks before production deployment:

```bash
make preflight
# Or manually:
./ops/preflight-check.sh
```

This verifies:
1. **Time Synchronization:** NTP enabled, clock drift < 1 second
2. **RPC Latency:** Average latency to Helius < 50ms
3. **Circuit Breaker:** Automatic halt functionality working

### Production Deployment

1. **Build release binaries:**
   ```bash
   make build
   ```

2. **Run preflight checks:**
   ```bash
   make preflight
   ```

3. **Install systemd service:**
   ```bash
   sudo make install-service
   ```

4. **Deploy:**
   ```bash
   make deploy
   # Or use rsync:
   make deploy-rsync SERVER=user@your-server
   ```

5. **Verify deployment:**
   ```bash
   curl http://your-server:8080/api/v1/health
   ```

See [Pre-Deployment Checklist](docs/pre-deployment-checklist.md) for complete deployment procedures.

---

## 📊 Monitoring & Observability

### Prometheus Metrics

Metrics are exposed at `/metrics` endpoint:

- Queue depth and latency
- Trade execution metrics
- Circuit breaker state
- RPC health and latency
- Reconciliation metrics
- Secret rotation tracking

### Grafana Dashboard

Import the dashboard from `ops/grafana/dashboard.json` to visualize:
- System health status
- Queue depth and performance
- Active positions and PnL
- RPC health and latency
- Strategy breakdown

### Alertmanager

Configure alerts in `ops/prometheus/alerts.yml` for:
- Queue backpressure
- Trade latency spikes
- Circuit breaker triggers
- Reconciliation discrepancies
- Secret rotation failures

### Notifications

Configure Telegram or Discord notifications for:
- Circuit breaker triggers
- Wallet promotions/demotions
- Position exits
- Daily trading summaries
- RPC fallback events

See [Notifications Setup](docs/notifications-setup.md) for configuration.

---

## 🔐 Security

### Best Practices

1. **Secret Management:**
   - Use encrypted vault for private keys
   - Rotate webhook secrets regularly
   - Never commit secrets to version control

2. **Network Security:**
   - Use firewall rules to restrict access
   - Enable HTTPS/TLS for production
   - Use VPN or private network for RPC endpoints

3. **Access Control:**
   - Configure API keys with appropriate roles
   - Use wallet-based authentication for dashboard
   - Implement rate limiting on all endpoints

4. **Audit & Compliance:**
   - Review `config_audit` table regularly
   - Monitor `dead_letter_queue` for failures
   - Run daily reconciliation checks

See [Security Audit Checklist](docs/security-audit-checklist.md) for complete security procedures.

---

## 🐛 Troubleshooting

### Common Issues

**Issue: Webhook rejected with "circuit_breaker_triggered"**
- **Solution:** Check circuit breaker state via `/api/v1/health`. Reset if needed via `/api/v1/config/circuit-breaker/reset` (admin only).

**Issue: High latency to RPC endpoints**
- **Solution:** Verify server location is in US-East or Amsterdam. Check network routing and consider alternative RPC providers.

**Issue: Database locked errors**
- **Solution:** Ensure WAL mode is enabled. Check for long-running transactions. Review `ops/runbooks/sqlite_lock.md`.

**Issue: Scout not updating roster**
- **Solution:** Verify Scout has write permissions. Check `roster_new.db` exists. Trigger merge via `kill -HUP` or API call.

See [Runbooks](ops/runbooks/) for detailed troubleshooting procedures.

---

## 📝 License

Project Chimera is proprietary software. Unauthorized copying or distribution is strictly prohibited.

---

## 🤝 Contributing

This is a private project. For questions or support, please contact the maintainers.

---

## 📞 Support

- **Documentation:** See `docs/` directory
- **Runbooks:** See `ops/runbooks/` for incident response
- **Issues:** Contact project maintainers

---

## 🎯 Roadmap

Future enhancements (non-blocking):
- Direct Jito Searcher integration (currently uses Helius Sender API)
- Enhanced Raydium/Orca pool enumeration
- Advanced historical liquidity database
- Property-based testing expansion

---

<div align="center">

**Built with ❤️ for high-frequency Solana trading**

[Documentation](docs/) • [API Reference](docs/api.md) • [Architecture](docs/architecture.md)

</div>
