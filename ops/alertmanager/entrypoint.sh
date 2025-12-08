#!/bin/sh
set -e

# Substitute environment variables in config file using sed
# Alertmanager doesn't support env var substitution natively
# chat_id must be an integer (no quotes), bot_token is a string (keep quotes)
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"
TELEGRAM_CHAT_ID="${TELEGRAM_CHAT_ID:-}"

# Replace variables: bot_token keeps quotes, chat_id is integer without quotes
sed -e "s|\${TELEGRAM_BOT_TOKEN}|${TELEGRAM_BOT_TOKEN}|g" \
    -e "s|chat_id: '\${TELEGRAM_CHAT_ID}'|chat_id: ${TELEGRAM_CHAT_ID}|g" \
    < /etc/alertmanager/config.yml.template > /tmp/config.yml

# Start Alertmanager with the substituted config
exec /bin/alertmanager \
  --config.file=/tmp/config.yml \
  --storage.path=/alertmanager \
  "$@"
