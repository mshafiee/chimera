#!/bin/bash
# Chimera ngrok Setup Script
# Run this after you get your ngrok authtoken

echo "=== Chimera ngrok Setup ==="
echo ""

# Check if authtoken provided
if [ -z "$1" ]; then
    echo "Usage: ./setup-ngrok.sh YOUR_NGROK_AUTHTOKEN"
    echo "Get your authtoken from: https://dashboard.ngrok.com/get-started/your-authtoken"
    exit 1
fi

NGROK_TOKEN="$1"

echo "Step 1: Configuring ngrok..."
ngrok config add-authtoken "$NGROK_TOKEN"

if [ $? -eq 0 ]; then
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

# Extract the public URL from ngrok
echo "Step 4: Extracting public URL..."
NGROK_URL=$(curl -s http://127.0.0.1:4040/api/tunnels | grep -o '"public_url":"[^"]*' | grep -o 'https://[^"]*' | head -1)

if [ -z "$NGROK_URL" ]; then
    echo "✗ Failed to get ngrok URL. Check ngrok.log for details"
    kill $NGROK_PID
    exit 1
fi

echo "✓ ngrok tunnel URL: $NGROK_URL"
echo ""

# Update configuration files
echo "Step 5: Updating Chimera configuration..."

# Backup original files
cp .env .env.backup
cp config/config.yaml config/config.yaml.backup

# Update .env file
if grep -q "CHIMERA_MONITORING__HELIUS_WEBHOOK_URL" .env; then
    sed -i '' "s|CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=.*|CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=${NGROK_URL}/api/v1/monitoring/helius-webhook|" .env
else
    echo "CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=${NGROK_URL}/api/v1/monitoring/helius-webhook" >> .env
fi

# Update config.yaml
sed -i '' "s|helius_webhook_url: \".*\"|helius_webhook_url: \"${NGROK_URL}/api/v1/monitoring/helius-webhook\"|" config/config.yaml

echo "✓ Configuration files updated"
echo ""

# Restart operator
echo "Step 6: Restarting Chimera operator..."
OPERATOR_PID=$(pgrep chimera_operator)
if [ -n "$OPERATOR_PID" ]; then
    kill -HUP $OPERATOR_PID
    echo "✓ Operator restarted (PID: $OPERATOR_PID)"
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
echo "2. Update Helius webhooks (see WEBHOOK_SETUP_GUIDE.md)"
echo "3. Monitor logs: tail -f operator/operator.log | grep webhook"
echo ""
echo "Note: ngrok tunnel is running in background (PID: ${NGROK_PID})"
echo "To stop ngrok later: kill ${NGROK_PID}"