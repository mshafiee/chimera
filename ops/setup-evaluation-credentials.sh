#!/bin/bash
# Quick setup helper for evaluation credentials

echo "🔧 Chimera 10-Day Evaluation - Credential Setup Helper"
echo "====================================================="
echo ""

# Generate webhook secret
echo "1. Generating Webhook Secret..."
WEBHOOK_SECRET=$(openssl rand -hex 32)
echo "   Generated secret: ${WEBHOOK_SECRET:0:16}..."
echo ""

# Suggest secure passwords
echo "2. Generating secure passwords..."
POSTGRES_PASSWORD=$(openssl rand -base64 16 | tr -d '=+/' | cut -c1-16)
GRAFANA_PASSWORD=$(openssl rand -base64 16 | tr -d '=+/' | cut -c1-16)
echo "   PostgreSQL password: ${POSTGRES_PASSWORD}"
echo "   Grafana password: ${GRAFANA_PASSWORD}"
echo ""

# Create template
echo "3. Creating configuration template..."
cat > docker/env.evaluation.local << EOF
# Chimera Evaluation Local Configuration
# Generated: $(date)

# ===================================================================
# HELIUS API KEY - REQUIRED FOR MAINNET EVALUATION
# ===================================================================
# Get your API key from: https://www.helius.dev/
HELIUS_API_KEY=your_helius_api_key_here

# ===================================================================
# NOTIFICATION CONFIGURATION
# ===================================================================
# Telegram Bot Configuration (optional but recommended for evaluation alerts)
TELEGRAM_BOT_TOKEN=your_telegram_bot_token_here
TELEGRAM_CHAT_ID=your_telegram_chat_id_here

# Discord Webhook Configuration (optional)
DISCORD_WEBHOOK_URL=your_discord_webhook_url_here

# ===================================================================
# EVALUATION DATABASE CREDENTIALS
# ===================================================================
# These credentials are for the evaluation PostgreSQL database
POSTGRES_EVAL_PASSWORD=${POSTGRES_PASSWORD}

# ===================================================================
# GRAFANA ADMIN CREDENTIALS
# ===================================================================
# Admin password for evaluation Grafana dashboards
GRAFANA_ADMIN_PASSWORD=${GRAFANA_PASSWORD}

# ===================================================================
# WEBHOOK SECRET FOR EVALUATION
# ===================================================================
# Auto-generated webhook secret for signal authentication
CHIMERA_SECURITY__WEBHOOK_SECRET=${WEBHOOK_SECRET}

# ===================================================================
# SIGNAL PROVIDER CONFIGURATION (for Days 6-10)
# ===================================================================
# If using real-time signals from external providers
SIGNAL_PROVIDER_URL=https://your-signal-provider.com/webhook
SIGNAL_PROVIDER_API_KEY=your_signal_provider_api_key_if_needed
EOF

echo "   ✅ Template created: docker/env.evaluation.local"
echo ""

# Provide instructions
echo "📋 Next Steps:"
echo "1. Edit docker/env.evaluation.local"
echo "2. Replace 'your_helius_api_key_here' with your actual Helius API key"
echo "3. (Optional) Add Telegram/Discord credentials for notifications"
echo "4. Save the file"
echo ""
echo "⚠️  IMPORTANT: Get Helius API key from https://www.helius.dev/"
echo "   (Sign up for free developer account - sufficient for evaluation)"
echo ""