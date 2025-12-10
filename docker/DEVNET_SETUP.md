# Devnet Setup Guide - Required Data

This guide explains what data you need to provide to run Chimera on Devnet.

## ✅ Minimum Required (Can Start Immediately)

For **basic testing**, you can start with the default devnet configuration:

```bash
# 1. Initialize database
./docker/docker-compose.sh init-db devnet

# 2. Start services (uses default config)
./docker/docker-compose.sh start devnet
```

The default `docker/env.devnet` file includes:
- ✅ Public Devnet RPC endpoint (no API key needed)
- ✅ Default webhook secret (for testing only)
- ✅ All other settings pre-configured

## 🔧 Recommended Configuration

For **proper testing**, you should customize these values:

### 1. Webhook Secret (Recommended)

Generate a secure webhook secret:

```bash
openssl rand -hex 32
```

Then create a local override file:

```bash
# Copy the template
cp docker/env.devnet docker/env.devnet.local

# Edit the file and update:
# CHIMERA_SECURITY__WEBHOOK_SECRET=<your-generated-secret>
```

### 2. Optional: Enhanced RPC (If you have Helius Devnet API key)

If you have a Helius API key for devnet, you can use it for better rate limits:

```bash
# In docker/env.devnet.local, update:
CHIMERA_RPC__PRIMARY_URL=https://devnet.helius-rpc.com/?api-key=YOUR_DEVNET_API_KEY
```

**Note:** The public devnet endpoint works fine for testing, but has lower rate limits.

### 3. Optional: Notifications

For testing notifications, add Telegram or Discord:

```bash
# In docker/env.devnet.local:
TELEGRAM_BOT_TOKEN=your-bot-token
TELEGRAM_CHAT_ID=your-chat-id

# OR

DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

## 📋 Complete Configuration Checklist

Here's what you can configure in `docker/env.devnet.local`:

### Required (for production-like testing)
- [ ] `CHIMERA_SECURITY__WEBHOOK_SECRET` - Generate with `openssl rand -hex 32`

### Optional (but recommended)
- [ ] `CHIMERA_RPC__PRIMARY_URL` - Use Helius devnet endpoint if you have API key
- [ ] `TELEGRAM_BOT_TOKEN` - For notifications
- [ ] `TELEGRAM_CHAT_ID` - For notifications
- [ ] `DISCORD_WEBHOOK_URL` - Alternative to Telegram
- [ ] `GRAFANA_PASSWORD` - Change from default "admin"

### Already Configured (no changes needed)
- ✅ `CHIMERA_RPC__PRIMARY_URL` - Public devnet endpoint
- ✅ `CHIMERA_DEV_MODE=1` - Enables dev mode (relaxed validation)
- ✅ Circuit breaker thresholds (relaxed for devnet)
- ✅ Strategy configuration (70% Shield, 30% Spear)
- ✅ Token safety thresholds (relaxed for devnet)
- ✅ Database path
- ✅ All other settings

## 🚀 Quick Start Steps

### Option 1: Minimal Setup (Fastest)

```bash
# 1. Initialize database
./docker/docker-compose.sh init-db devnet

# 2. Start with defaults
./docker/docker-compose.sh start devnet

# 3. Check status
./docker/docker-compose.sh status devnet

# 4. View logs
./docker/docker-compose.sh logs devnet -f
```

### Option 2: Customized Setup (Recommended)

```bash
# 1. Generate webhook secret
openssl rand -hex 32
# Copy the output

# 2. Create local config
cp docker/env.devnet docker/env.devnet.local

# 3. Edit docker/env.devnet.local
#    - Paste your webhook secret
#    - (Optional) Add Helius API key
#    - (Optional) Add Telegram/Discord webhooks

# 4. Initialize database
./docker/docker-compose.sh init-db devnet

# 5. Start services
./docker/docker-compose.sh start devnet
```

## 🔍 Verify Your Setup

After starting, verify everything is working:

```bash
# 1. Check operator health
curl http://localhost:8080/api/v1/health

# 2. Check web dashboard
open http://localhost:3000

# 3. Check Grafana
open http://localhost:3002
# Login: admin / admin (or your custom password)

# 4. Check Prometheus
open http://localhost:9090
```

## 📝 Example Configuration File

Here's an example `docker/env.devnet.local` with all recommended settings:

```bash
# Webhook Secret (REQUIRED - generate with: openssl rand -hex 32)
CHIMERA_SECURITY__WEBHOOK_SECRET=your-64-character-hex-secret-here

# Enhanced RPC (Optional - if you have Helius devnet API key)
CHIMERA_RPC__PRIMARY_URL=https://devnet.helius-rpc.com/?api-key=YOUR_DEVNET_KEY

# Notifications (Optional)
TELEGRAM_BOT_TOKEN=123456789:ABCdefGHIjklMNOpqrsTUVwxyz
TELEGRAM_CHAT_ID=123456789

# OR Discord
# DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/123456789/abcdefgh

# Grafana Password (Optional - change from default)
GRAFANA_PASSWORD=my-secure-password
```

## ⚠️ Important Notes

1. **Webhook Secret**: The default secret in `docker/env.devnet` is for testing only. Generate a new one for any real testing.

2. **API Keys**: Devnet doesn't require an API key for the public endpoint, but using Helius gives you better rate limits.

3. **Dev Mode**: `CHIMERA_DEV_MODE=1` is enabled by default, which skips some validations. This is fine for devnet testing.

4. **Database**: The database is created automatically in `./data/chimera.db` when you initialize.

5. **Ports**: Make sure ports 8080, 3000, 3002, 9090, and 9093 are available.

## 🐛 Troubleshooting

**Services won't start:**
```bash
# Check logs
./docker/docker-compose.sh logs devnet

# Verify config
docker-compose --profile devnet config
```

**Database errors:**
```bash
# Reinitialize database
rm -f data/chimera.db*
./docker/docker-compose.sh init-db devnet
```

**Port conflicts:**
Edit `docker-compose.yml` to change port mappings if needed.

## 📚 Next Steps

Once running:
1. Test webhook endpoint: `POST http://localhost:8080/api/v1/webhook`
2. Explore web dashboard: http://localhost:3000
3. Monitor metrics in Grafana: http://localhost:3002
4. Review logs: `./docker/docker-compose.sh logs devnet -f`
