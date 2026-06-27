# Notification Setup Guide

This guide explains how to configure Telegram and Discord notifications for Chimera.

## Overview

Chimera supports push notifications via:
- **Telegram Bot** - Real-time alerts via Telegram messages
- **Discord Webhook** - Rich embed messages in Discord channels

Both notification methods support:
- Rate limiting to prevent spam
- Alert level filtering (Critical, Important, Info)
- Configurable notification rules

---

## Telegram Bot Setup

### Step 1: Create a Telegram Bot

1. Open Telegram and search for **@BotFather**
2. Start a conversation with BotFather
3. Send the command: `/newbot`
4. Follow the prompts:
   - Choose a name for your bot (e.g., "Chimera Alerts")
   - Choose a username (must end in `bot`, e.g., `chimera_alerts_bot`)
5. BotFather will provide you with a **bot token** (looks like: `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`)

**Save this token securely** - you'll need it for configuration.

### Step 2: Get Your Chat ID

You need to know which chat to send messages to. There are two methods:

#### Method A: Using @userinfobot (Recommended)

1. Search for **@userinfobot** in Telegram
2. Start a conversation - it will reply with your user ID
3. Your chat ID is the number shown (e.g., `123456789`)

#### Method B: Using @getidsbot

1. Search for **@getidsbot** in Telegram
2. Add it to a group or start a private chat
3. It will display the chat ID

**For group chats:** The chat ID will be negative (e.g., `-1001234567890`)

### Step 3: Configure Environment Variables

Set the following environment variables:

```bash
export TELEGRAM_BOT_TOKEN="your-bot-token-from-botfather"
export TELEGRAM_CHAT_ID="your-chat-id"
```

Or add to your `.env` file:

```env
TELEGRAM_BOT_TOKEN=123456789:ABCdefGHIjklMNOpqrsTUVwxyz
TELEGRAM_CHAT_ID=123456789
```

### Step 4: Update Configuration File

The configuration file (`config/config.yaml`) should have:

```yaml
notifications:
  telegram:
    enabled: true
    bot_token: "${TELEGRAM_BOT_TOKEN}"  # From environment
    chat_id: "${TELEGRAM_CHAT_ID}"     # From environment
    rate_limit_seconds: 60  # Minimum seconds between similar messages
```

### Step 5: Test Notifications

1. Start the Chimera operator
2. Trigger a test notification (e.g., by promoting a wallet via API)
3. Check your Telegram chat for the notification

**Troubleshooting:**
- If no messages appear, verify:
  - Bot token is correct
  - Chat ID is correct (including negative sign for groups)
  - Bot is not blocked
  - Environment variables are set correctly

---

## Discord Webhook Setup

### Step 1: Create a Discord Webhook

1. Open Discord and navigate to your server
2. Go to **Server Settings** → **Integrations** → **Webhooks**
3. Click **New Webhook**
4. Configure:
   - **Name**: "Chimera Alerts" (or your preferred name)
   - **Channel**: Select the channel for notifications
   - **Avatar**: Optional, upload a custom avatar
5. Click **Copy Webhook URL**

The webhook URL looks like:
```
https://discord.com/api/webhooks/123456789012345678/abcdefghijklmnopqrstuvwxyz1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ
```

**Save this URL securely** - you'll need it for configuration.

### Step 2: Configure Environment Variable

Set the following environment variable:

```bash
export DISCORD_WEBHOOK_URL="your-webhook-url-from-discord"
```

Or add to your `.env` file:

```env
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/123456789012345678/abcdefghijklmnopqrstuvwxyz1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ
```

### Step 3: Update Configuration File

Discord notifications are automatically enabled when `DISCORD_WEBHOOK_URL` is set. No additional configuration is needed in `config.yaml`.

### Step 4: Test Notifications

1. Start the Chimera operator
2. Trigger a test notification (e.g., by promoting a wallet via API)
3. Check your Discord channel for the notification

**Troubleshooting:**
- If no messages appear, verify:
  - Webhook URL is correct and complete
  - Webhook is not deleted or disabled
  - Channel permissions allow webhook messages
  - Environment variable is set correctly

---

## Notification Rules Configuration

You can control which events trigger notifications via the `notifications.rules` section in `config.yaml`:

```yaml
notifications:
  rules:
    circuit_breaker_triggered: true   # Critical: Circuit breaker trips
    wallet_drained: true              # Critical: Wallet balance drops significantly
    position_exited: true             # Important: Position closed (profit/loss)
    wallet_promoted: true              # Info: Wallet promoted to ACTIVE
    daily_summary: true                # Info: Daily trading summary
    rpc_fallback: true                 # Important: Switched to fallback RPC
```

**Default:** All rules are enabled by default.

**Alert Levels:**
- **Critical** (🔴): Circuit breaker, wallet drained, system crash
- **Important** (🟡): Position exited, RPC fallback
- **Info** (🔵): Wallet promoted, daily summary

### Rate Limiting

Notifications are rate-limited to prevent spam:
- **Telegram**: Configurable via `rate_limit_seconds` (default: 60 seconds)
- **Discord**: Same rate limiting applies
- **Critical alerts**: Always sent, bypass rate limits
- **Important/Info alerts**: Subject to rate limiting

Rate limiting is per notification type. For example:
- Multiple position exits for different tokens will be rate-limited separately
- Circuit breaker alerts always bypass rate limits

---

## Daily Summary Configuration

Configure when daily summaries are sent:

```yaml
notifications:
  daily_summary:
    enabled: true
    hour_utc: 20    # 8 PM UTC
    minute: 0
```

Daily summaries include:
- Total PnL (USD)
- Number of trades
- Win rate percentage

---

## Notification Examples

### Circuit Breaker Triggered
```
🔴 CRITICAL

🚨 Circuit breaker triggered: Max loss exceeded (24h: -$550.00)
```

### Position Exited
```
🟡 IMPORTANT

💰 BONK SHIELD: +25.00% (+0.1250 SOL)
```

### Wallet Promoted
```
🔵 INFO

📊 Wallet promoted: 7xKX...gAsU (WQS: 85.30)
```

### Daily Summary
```
🔵 INFO

📈 Daily: +$127.50 USD | Trades: 12 | Win: 75.0%
```

---

## Security Considerations

1. **Bot Tokens & Webhook URLs are Secrets**
   - Never commit them to version control
   - Use environment variables or encrypted config
   - Rotate tokens/URLs if compromised

2. **Chat ID Privacy**
   - Chat IDs can be used to send messages to your chats
   - Keep them secure like other secrets

3. **Webhook URL Security**
   - Anyone with the webhook URL can send messages to your channel
   - If exposed, delete and recreate the webhook immediately

4. **Rate Limiting**
   - Rate limiting prevents abuse but may delay non-critical alerts
   - Critical alerts always bypass rate limits

---

## Troubleshooting

### Telegram: "Unauthorized" Error
- Verify bot token is correct
- Ensure bot is not deleted or disabled

### Telegram: "Chat not found"
- Verify chat ID is correct
- For groups, ensure bot is added to the group
- For private chats, ensure you've started a conversation with the bot

### Discord: "Invalid Webhook"
- Verify webhook URL is complete and correct
- Check if webhook was deleted in Discord settings
- Ensure webhook channel still exists

### No Notifications Received
1. Check notification rules in `config.yaml` - ensure relevant rule is `true`
2. Verify environment variables are set correctly
3. Check operator logs for notification errors
4. Verify rate limiting isn't blocking notifications (critical alerts bypass this)
5. Test with a critical alert (circuit breaker) to verify setup

### Too Many Notifications
1. Adjust rate limiting: Increase `rate_limit_seconds` in config
2. Disable specific notification rules in `config.yaml`
3. Use notification rules to filter by alert level

---

## Advanced Configuration

### Using Both Telegram and Discord

You can enable both notification methods simultaneously:

```bash
export TELEGRAM_BOT_TOKEN="your-token"
export TELEGRAM_CHAT_ID="your-chat-id"
export DISCORD_WEBHOOK_URL="your-webhook-url"
```

Both will receive notifications for all enabled rules.

### Custom Notification Filtering

To implement custom filtering logic, modify `operator/src/main.rs` where notifications are sent. The notification system checks `NotificationRulesConfig` before sending.

---

## Support

For issues or questions:
1. Check operator logs: `journalctl -u chimera -n 50`
2. Verify configuration: `cat config/config.yaml | grep -A 20 notifications`
3. Test environment variables: `env | grep TELEGRAM\|DISCORD`
4. Review runbooks in `ops/runbooks/`
