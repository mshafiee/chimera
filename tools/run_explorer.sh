#!/bin/bash
# Telegram Signal Explorer - Interactive Run Script

echo "=========================================="
echo "  Telegram Signal Explorer"
echo "=========================================="
echo ""
echo "This script will analyze 16 Telegram channels for trading signals."
echo "First-time setup requires phone number verification."
echo ""

# Check if credentials are set
if [ -z "$TELEGRAM_API_ID" ] || [ -z "$TELEGRAM_API_HASH" ]; then
    echo "ERROR: Telegram API credentials not set!"
    echo ""
    echo "Please run with your credentials:"
    echo "  TELEGRAM_API_ID=23096656 TELEGRAM_API_HASH=76b1d46305d224598881ce45f861b473 bash run_explorer.sh"
    echo ""
    exit 1
fi

echo "Using API ID: $TELEGRAM_API_ID"
echo ""

# Run the explorer
python3 telegram_explorer.py --config telegram_config.yaml

echo ""
echo "=========================================="
echo "Analysis complete! Check telegram_analysis/ for results."
echo "=========================================="
