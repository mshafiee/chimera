#!/bin/bash
# Chimera Evaluation Alert Configuration Setup Helper
# Guides users through setting up Telegram and Discord notification tokens

set -e

echo "==================================="
echo "Chimera Alert Configuration Setup"
echo "==================================="
echo ""
echo "This helper script will guide you through configuring"
echo "real-time notifications for your evaluation environment."
echo ""

# Check if we're in the right directory
if [ ! -f "docker-compose.evaluation.yml" ]; then
    echo "❌ Error: Please run this script from the chimera root directory"
    echo "   Expected file: docker-compose.evaluation.yml"
    exit 1
fi

# Load existing configuration if available
if [ -f "docker/env.evaluation" ]; then
    echo "📋 Loading existing configuration from docker/env.evaluation..."
    source docker/env.evaluation 2>/dev/null || true
else
    echo "📋 No existing configuration found"
fi

# Function to check and display token status
check_token_status() {
    local token_name=$1
    local token_value=$2
    local setup_instructions=$3

    if [ -n "$token_value" ] && [ "$token_value" != "changeme" ] && [ "$token_value" != "your_token_here" ]; then
        echo "✅ $token_name: CONFIGURED"
        return 0
    else
        echo "❌ $token_name: NOT CONFIGURED"
        if [ -n "$setup_instructions" ]; then
            echo "   $setup_instructions"
        fi
        return 1
    fi
}

# Check current configuration status
echo ""
echo "Current Configuration Status:"
echo "----------------------------"

telegram_configured=false
discord_configured=false

if check_token_status "TELEGRAM_BOT_TOKEN" "$TELEGRAM_BOT_TOKEN" "Get token from @BotFather in Telegram"; then
    telegram_configured=true
fi

if check_token_status "TELEGRAM_CHAT_ID" "$TELEGRAM_CHAT_ID" "Get ID from @userinfobot or @getidsbot in Telegram"; then
    : # Token is configured
fi

if check_token_status "DISCORD_WEBHOOK_URL" "$DISCORD_WEBHOOK_URL" "Create webhook in Server Settings → Integrations"; then
    discord_configured=true
fi

echo ""
echo "Setup Instructions:"
echo "------------------"

if [ "$telegram_configured" = false ]; then
    echo ""
    echo "📱 Telegram Setup:"
    echo "1. Open Telegram and search for @BotFather"
    echo "2. Send /newbot and follow the prompts"
    echo "3. Copy the bot token (format: 123456789:ABCdefGHIjklMNOpqrsTUVwxyz)"
    echo "4. Search for @userinfobot or @getidsbot"
    echo "5. Send /start to get your chat ID"
    echo ""
    echo "Add to docker/env.evaluation:"
    echo "TELEGRAM_BOT_TOKEN=your_bot_token_here"
    echo "TELEGRAM_CHAT_ID=your_chat_id_here"
fi

if [ "$discord_configured" = false ]; then
    echo ""
    echo "💬 Discord Setup:"
    echo "1. Open your Discord server settings"
    echo "2. Go to Integrations → Webhooks"
    echo "3. Create new webhook"
    echo "4. Copy the webhook URL"
    echo ""
    echo "Add to docker/env.evaluation:"
    echo "DISCORD_WEBHOOK_URL=your_webhook_url_here"
fi

echo ""
echo "🔒 Security Reminder:"
echo "------------------"
echo "⚠️  Never commit actual tokens to version control!"
echo "   docker/env.evaluation is in .gitignore"
echo ""
echo "🔒 Best Practices:"
echo "   • Rotate tokens every 30-90 days"
echo "   • Use different tokens for dev/prod"
echo "   • Monitor for unauthorized access"
echo "   • Revoke compromised tokens immediately"

echo ""
echo "Configuration complete! 🚀"