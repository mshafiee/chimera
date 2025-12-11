#!/bin/sh
set -e

# Substitute environment variables in config file using sed
# Alertmanager doesn't support env var substitution natively
# chat_id must be an integer (no quotes), bot_token is a string (keep quotes)
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"
TELEGRAM_CHAT_ID="${TELEGRAM_CHAT_ID:-}"

# If chat_id is not set or is a placeholder, create a minimal config without Telegram
if [ -z "$TELEGRAM_CHAT_ID" ] || [ "$TELEGRAM_CHAT_ID" = "your-telegram-chat-id" ] || ! echo "$TELEGRAM_CHAT_ID" | grep -qE '^[0-9]+$'; then
    # Create minimal config without Telegram receivers
    cat > /tmp/config.yml << 'EOF'
global:
  resolve_timeout: 5m

route:
  receiver: 'null'
  group_by: ['alertname']
  group_wait: 10s
  group_interval: 10s
  repeat_interval: 12h

receivers:
  - name: 'null'
EOF
else
    # Replace variables: bot_token keeps quotes, chat_id is integer without quotes
    sed -e "s|\${TELEGRAM_BOT_TOKEN}|${TELEGRAM_BOT_TOKEN}|g" \
        -e "s|chat_id: '\${TELEGRAM_CHAT_ID}'|chat_id: ${TELEGRAM_CHAT_ID}|g" \
        < /etc/alertmanager/config.yml.template > /tmp/config.yml
fi

# Start Alertmanager with the substituted config
exec /bin/alertmanager \
  --config.file=/tmp/config.yml \
  --storage.path=/alertmanager \
  "$@"
