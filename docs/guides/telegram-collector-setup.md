# Telegram Signal Collector Setup Guide

This guide covers setting up the Telegram Signal Collector service for Chimera.

## Prerequisites

1. **Telegram API Credentials**
   - Visit https://my.telegram.org
   - Sign in with your phone number
   - Create a new application to get `api_id` and `api_hash`

2. **Install Dependencies**
   ```bash
   cd scout
   pip install -r requirements.txt
   ```

## Configuration

Set the following environment variables:

```bash
export TELEGRAM_API_ID="your_api_id"
export TELEGRAM_API_HASH="your_api_hash"
export CHIMERA_OPERATOR_URL="http://localhost:8080"
```

Optionally, set an API key if authentication is enabled:

```bash
export CHIMERA_API_KEY="your_api_key"
```

## Running the Collector

### Development Mode (Dry Run)

Test without sending signals to the operator:

```bash
cd scout
python telegram_collector.py --config ../config/config.yaml --dry-run
```

### Production Mode

Monitor channels and send signals to the operator:

```bash
cd scout
python telegram_collector.py --config ../config/config.yaml
```

### Custom Channels

Monitor specific channels:

```bash
python telegram_collector.py --channels "@channel1,@channel2"
```

## Default Channels

The collector monitors these high-value channels by default:
- `@solana_whales_signal`
- `@SolmemeWhaleinsider`
- `@SolanaDaily_Pumps`

## Signal Flow

```
Telegram Message → SignalParser → Operator API (/api/v1/telegram/signal) → Trading Engine
```

## Monitoring

Check collector status:
```bash
curl http://localhost:8080/api/v1/telegram/status
```

Expected output:
```json
{
  "data": {
    "enabled_channels": 3,
    "channels": [
      {
        "channel_id": "@solana_whales_signal",
        "enabled": true,
        "is_healthy": true,
        "parse_success_rate": 1.0
      }
    ]
  }
}
```

## Troubleshooting

### "Telegram API credentials not found"
- Ensure `TELEGRAM_API_ID` and `TELEGRAM_API_HASH` are set
- Verify credentials from https://my.telegram.org

### "Cannot access private channel"
- You must join the channel first with your Telegram account
- Some channels require approval from admins

### "Operator rejected signal: 404"
- Verify `CHIMERA_OPERATOR_URL` is correct
- Check that the operator is running
- Ensure telegram_sources is enabled in config.yaml

### "Operator rejected signal: 500"
- Check operator logs for errors
- Verify database migration was applied
- Check if circuit breaker is tripped

## Security Notes

1. **API Credentials**: Never commit `TELEGRAM_API_HASH` to version control
2. **Session Files**: The collector creates `telegram_collector.session` - protect this file
3. **Network**: Use HTTPS in production for operator communication

## Production Deployment

### Systemd Service

Create `/etc/systemd/system/chimera-telegram.service`:

```ini
[Unit]
Description=Chimera Telegram Collector
After=network.target chimera-operator.service

[Service]
Type=simple
User=chimera
WorkingDirectory=/opt/chimera/scout
Environment="TELEGRAM_API_ID=%i"
Environment="TELEGRAM_API_HASH=%j"
Environment="CHIMERA_OPERATOR_URL=http://localhost:8080"
ExecStart=/usr/bin/python3 telegram_collector.py
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl enable chimera-telegram@API_ID.service
sudo systemctl start chimera-telegram@API_ID.service
```

## Gradual Rollout

1. **Shadow Mode** (Week 1-2)
   - Enable signal ingestion only
   - Set `dry_run: true` in operator config
   - Validate parsing quality

2. **Paper Trading** (Week 3-4)
   - Enable with 0.01 SOL position sizes
   - Monitor performance metrics
   - Disable underperforming channels

3. **Limited Live** (Week 5-6)
   - Enable top 3 channels
   - 50% position sizes
   - Require consensus with on-chain wallets

4. **Full Integration** (Week 7+)
   - Normal position sizes
   - All qualifying channels
   - Full consensus detection
