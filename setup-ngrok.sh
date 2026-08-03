#!/bin/bash
# Chimera ngrok Setup Script
# Run this after you get your ngrok authtoken

set -euo pipefail

echo "=== Chimera ngrok Setup ==="
echo ""

# Check if authtoken provided (env var preferred so it never appears in argv)
if [ -z "${NGROK_AUTHTOKEN:-}" ]; then
    if [ -n "${1:-}" ]; then
        NGROK_AUTHTOKEN="$1"
    else
        read -r -s -p "Enter your ngrok authtoken: " NGROK_AUTHTOKEN
        echo
    fi
fi

if [ -z "$NGROK_AUTHTOKEN" ]; then
    echo "Usage: ./setup-ngrok.sh YOUR_NGROK_AUTHTOKEN"
    echo "Get your authtoken from: https://dashboard.ngrok.com/get-started/your-authtoken"
    exit 1
fi

echo "Step 1: Configuring ngrok..."
if ngrok config add-authtoken "$NGROK_AUTHTOKEN"; then
    echo "✓ ngrok configured successfully"
else
    echo "✗ Failed to configure ngrok"
    exit 1
fi

echo ""
echo "Step 2: Starting ngrok tunnel on port 8080..."
echo "Note: Keep this terminal open - ngrok needs to stay running"
echo ""

# Start ngrok in background
ngrok http 8080 --log=stdout > ngrok.log 2>&1 &
NGROK_PID=$!

echo "✓ ngrok started with PID: $NGROK_PID"
echo ""
echo "Step 3: Waiting for ngrok to initialize..."
sleep 3

# Extract the public URL from ngrok, polling until the tunnel is ready
echo "Step 4: Extracting public URL..."
NGROK_URL=""
for i in $(seq 1 15); do
    NGROK_URL=$(curl -s --max-time 3 http://127.0.0.1:4040/api/tunnels 2>/dev/null | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    for t in data.get('tunnels', []):
        if t.get('proto') == 'https' and t.get('public_url'):
            print(t['public_url'])
            break
except Exception:
    pass
" 2>/dev/null || true)
    [ -n "$NGROK_URL" ] && break
    sleep 1
done

if [ -z "$NGROK_URL" ]; then
    echo "✗ Failed to get ngrok URL. Check ngrok.log for details"
    kill "$NGROK_PID" 2>/dev/null || true
    exit 1
fi

echo "✓ ngrok tunnel URL: $NGROK_URL"
echo ""

# Update configuration files
echo "Step 5: Updating Chimera configuration..."

# Backup original files
cp .env .env.backup
cp config/config.yaml config/config.yaml.backup

# Update .env file (replace or append, then verify)
if grep -qE '^[[:space:]]*CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=' .env; then
    sed -i.bak "s|^[[:space:]]*CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=.*|CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=${NGROK_URL}/api/v1/monitoring/helius-webhook|" .env
    rm -f .env.bak
else
    echo "CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=${NGROK_URL}/api/v1/monitoring/helius-webhook" >> .env
fi

# Update config.yaml (replace or append, then verify)
if grep -q "helius_webhook_url:" config/config.yaml; then
    sed -i.bak "s|helius_webhook_url: .*|helius_webhook_url: \"${NGROK_URL}/api/v1/monitoring/helius-webhook\"|" config/config.yaml
    rm -f config/config.yaml.bak
else
    echo "helius_webhook_url: \"${NGROK_URL}/api/v1/monitoring/helius-webhook\"" >> config/config.yaml
fi

if ! grep -q "${NGROK_URL}/api/v1/monitoring/helius-webhook" .env config/config.yaml; then
    echo "✗ Failed to update configuration" >&2
    exit 1
fi

echo "✓ Configuration files updated"
echo ""

# Restart operator
echo "Step 6: Restarting Chimera operator..."
OPERATOR_PID=$(pgrep -f "target/release/chimera_operator" || true)
if [ -n "$OPERATOR_PID" ]; then
    kill -HUP $OPERATOR_PID 2>/dev/null || true
    sleep 2
    if kill -0 $OPERATOR_PID 2>/dev/null; then
        echo "✓ Operator reloaded (PID: $OPERATOR_PID)"
    else
        echo "⚠ Operator exited after SIGHUP; relaunching..."
        cd operator && nohup ./target/release/chimera_operator > operator.log 2>&1 &
        echo "✓ Operator relaunched (PID: $!)"
    fi
else
    echo "⚠ No operator running - start it with: cd operator && ./target/release/chimera_operator"
fi

echo ""
echo "=== Setup Complete! ==="
echo ""
echo "Your public webhook URL is: ${NGROK_URL}/api/v1/monitoring/helius-webhook"
echo ""
echo "Next steps:"
echo "1. Test webhook: curl -X POST \"${NGROK_URL}/api/v1/monitoring/helius-webhook\" -H 'Content-Type: application/json' -d '{\"signature\":\"test\"}'"
echo "2. Update Helius webhooks (see docs/guides/webhook-setup.md)"
echo "3. Monitor logs: tail -f operator/operator.log | grep webhook"
echo ""
echo "Note: ngrok tunnel is running in background (PID: ${NGROK_PID})"
echo "To stop ngrok later: kill ${NGROK_PID}"
