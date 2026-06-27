# Telegram Signal Explorer

Tools for exploring and analyzing Telegram trading channels as part of Chimera's signal integration research.

## Overview

The `telegram_explorer.py` script reads messages from Telegram channels to determine if they provide valuable trading signals that could be integrated into Chimera's copy-trading system.

## Features

- **Signal Extraction**: Parses trading signals from Telegram messages
- **Multi-Format Support**: Handles text, links, and structured signal formats
- **Channel Analysis**: Calculates value scores for each channel
- **Performance Tracking**: Optional ROI tracking for signals
- **Report Generation**: JSON output with channel rankings and metrics

## Installation

1. **Get Telegram API Credentials**
   - Go to https://my.telegram.org/apps
   - Log in with your phone number
   - Create a new application to get `api_id` and `api_hash`

2. **Install Dependencies**
   ```bash
   cd tools
   pip install -r requirements.txt
   ```

3. **Configure**
   ```bash
   # Copy the example config
   cp telegram_config.yaml telegram_config.local.yaml
   
   # Edit with your credentials
   # Option 1: Set environment variables
   export TELEGRAM_API_ID="your_api_id"
   export TELEGRAM_API_HASH="your_api_hash"
   
   # Option 2: Edit telegram_config.yaml directly
   ```

## Usage

### Basic Usage

```bash
cd tools
python telegram_explorer.py --config telegram_config.yaml
```

### With Environment Variables

```bash
export TELEGRAM_API_ID="12345"
export TELEGRAM_API_HASH="abcdef123456"
python telegram_explorer.py
```

### Custom Configuration

```bash
python telegram_explorer.py --config path/to/config.yaml
```

## Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `api_id` | Telegram API ID | Required |
| `api_hash` | Telegram API Hash | Required |
| `session_name` | Telethon session name | `"telegram_explorer"` |
| `days_back` | Analysis period in days | `7` |
| `message_limit` | Messages per channel | `200` |
| `output_dir` | Output directory | `"tools/telegram_analysis"` |

## Output Files

The explorer generates three JSON files in the output directory:

### 1. `summary.json`
Complete analysis report with:
- Channel classifications (high/medium/low value)
- Detailed metrics for each channel
- Signal statistics

### 2. `raw_signals.json`
All extracted trading signals with:
- Token addresses
- Entry/target prices
- Confidence levels
- Timestamps

### 3. `rankings.json`
Channels ranked by value score (0-100)

## Channel Scoring

Channels are scored on five criteria:

| Criterion | Weight | Description |
|-----------|--------|-------------|
| Signal Frequency | 20% | ≥2 signals/day = full points |
| Parseability | 25% | ≥80% contain token address |
| Completeness | 20% | Has entry/liquidity info |
| Consistency | 15% | Regular posting schedule |
| Performance | 20% | Historical ROI (if available) |

## Value Classifications

- **🟢 HIGH VALUE (Score ≥70)**: Recommended for integration
- **🟡 MEDIUM VALUE (Score 40-69)**: Consider with manual approval
- **🔴 LOW VALUE (Score <40)**: Not recommended

## Signal Format Support

The explorer recognizes:
- Token addresses (base58 format)
- Token symbols ($SYMBOL)
- Entry and target prices
- Liquidity information
- Confidence indicators (emojis, keywords)

## Example Output

```
============================================================
TELEGRAM CHANNEL ANALYSIS SUMMARY
============================================================

Channels Analyzed: 16
Analysis Period: 7 days
Total Signals Found: 342

------------------------------------------------------------
CHANNEL RANKINGS
------------------------------------------------------------
 1. 🟢 @solana_whales_signal           - Score:  78.5 | Signals:  45 | Rate: 6.4/day
 2. 🟢 @SolmemeWhaleinsider            - Score:  72.3 | Signals:  38 | Rate: 5.4/day
 3. 🟡 @memespumper                    - Score:  58.1 | Signals:  22 | Rate: 3.1/day
...
```

## Troubleshooting

### "Session file needed"
- First run requires phone number verification
- Follow the prompts to enter your phone number and code

### "ChannelPrivateError"
- The channel is private and you need to join it first
- Or the channel username is incorrect

### Low Signal Detection
- Some channels use images for signals (not yet supported)
- Signals may use non-standard formats
- Check the raw_signals.json to see what was extracted

## Next Steps

After running the explorer:

1. **Review the rankings** - Identify top channels
2. **Check raw signals** - Verify parsing accuracy
3. **Decide on integration** - Based on high-value channels found

If valuable channels are found, proceed with full integration into Chimera's signal pipeline.

## Security Notes

- Store `telegram_config.yaml` securely (don't commit to git)
- Consider using environment variables for credentials
- The script creates a session file with your Telegram session
- Session file should be protected appropriately

## License

Part of the Chimera project. See main LICENSE file.
