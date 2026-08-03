#!/bin/sh
set -e

# Substitute environment variables in config file using sed
# Alertmanager doesn't support env var substitution natively
# chat_id must be an integer (no quotes), bot_token is a string (keep quotes)
umask 077
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"
TELEGRAM_CHAT_ID="${TELEGRAM_CHAT_ID:-}"

# If chat_id or bot_token is not set or is a placeholder, create a minimal config without Telegram
if [ -z "$TELEGRAM_CHAT_ID" ] || [ "$TELEGRAM_CHAT_ID" = "your-telegram-chat-id" ] \
    || ! echo "$TELEGRAM_CHAT_ID" | grep -qE '^[0-9]+$' \
    || [ -z "$TELEGRAM_BOT_TOKEN" ] || [ "$TELEGRAM_BOT_TOKEN" = "your-telegram-bot-token" ]; then
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
    # Escape sed replacement metacharacters in the values so & | \ are literal
    escaped_bot_token=$(printf '%s' "$TELEGRAM_BOT_TOKEN" | sed 's/[&|\\]/\\&/g')
    escaped_chat_id=$(printf '%s' "$TELEGRAM_CHAT_ID" | sed 's/[&|\\]/\\&/g')

    # Fail fast if the template is missing
    if [ ! -f /etc/alertmanager/config.yml.template ]; then
        echo "error: /etc/alertmanager/config.yml.template not found" >&2
        exit 1
    fi

    # Replace variables: bot_token keeps quotes, chat_id is integer without quotes
    sed -e "s|\${TELEGRAM_BOT_TOKEN}|${escaped_bot_token}|g" \
        -e "s|chat_id: '\${TELEGRAM_CHAT_ID}'|chat_id: ${escaped_chat_id}|g" \
        < /etc/alertmanager/config.yml.template > /tmp/config.yml

    # Fail fast if any placeholder survived substitution
    if grep -qE '\$\{[A-Z_]+\}' /tmp/config.yml; then
        echo "error: unresolved placeholders in rendered config" >&2
        exit 1
    fi
fi

# Start Alertmanager with the substituted config
exec /bin/alertmanager \
  --config.file=/tmp/config.yml \
  --storage.path=/alertmanager \
  "$@"
