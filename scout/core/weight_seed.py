"""WQS Weight Seed Loader.

Loads pre-seeded default weights from a JSON file, with env-var override
for the file path and fallback to the hardcoded defaults.
"""

import json
import logging
import os
from typing import Dict

logger = logging.getLogger(__name__)

# Default path relative to the scout package root
_DEFAULT_WEIGHTS_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "data",
    "wqs_default_weights.json",
)

# Hardcoded fallback used only when the JSON file cannot be loaded
_FALLBACK_WEIGHTS: Dict[str, float] = {
    "roi_score": 1.5,
    "win_rate_score": 1.2,
    "pf_score": 1.3,
    "sortino_score": 1.1,
    "drawdown_penalty": 1.2,
    "activity_score": 0.7,
    "recency_score": 1.0,
    "martingale_penalty": 1.5,
    "smart_money_score": 1.2,
    "entry_delay_score": 1.0,
    "pf_wr_penalty": 1.0,
    "token_diversity_score": 1.0,
    "dex_diversity_score": 1.0,
    "smart_money_removal": 1.0,
    "pump_spike_penalty": 1.0,
    "consistency_score": 1.0,
    "sniper_penalty": 1.0,
    "insider_penalty": 1.0,
    "scam_penalty": 1.0,
    "mev_risk_penalty": 1.0,
}


def get_weights_path() -> str:
    """Return the weights file path, respecting SCOUT_WQS_WEIGHTS_PATH env var."""
    return os.getenv("SCOUT_WQS_WEIGHTS_PATH", _DEFAULT_WEIGHTS_PATH)


def load_seeded_weights() -> Dict[str, float]:
    """Load pre-seeded weights from JSON file.

    Returns the MERGED weights dict on success: file entries override the
    hardcoded defaults, so every component always has an explicit weight.
    On any error (missing file, corrupt JSON, wrong types, encoding issues)
    logs a warning and returns the hardcoded fallback weights so the system
    never crashes from a bad weights file.
    """
    path = get_weights_path()
    try:
        # Explicit utf-8 so a non-UTF-8 bytes file can't raise
        # UnicodeDecodeError (a ValueError subclass outside the handlers)
        with open(path, encoding="utf-8") as f:
            raw = json.load(f)
        if not isinstance(raw, dict):
            logger.warning("Weight file %s did not contain a dict, using fallback", path)
            return dict(_FALLBACK_WEIGHTS)
        # Merge over the fallbacks so partial files never drop components
        result = dict(_FALLBACK_WEIGHTS)
        valid = 0
        for k, v in raw.items():
            try:
                result[k] = float(v)
                valid += 1
            except (TypeError, ValueError):
                logger.warning("Skipping non-numeric weight '%s' in %s", k, path)
                continue
        if valid == 0:
            logger.warning("Weight file %s contained no valid entries, using fallback", path)
            return dict(_FALLBACK_WEIGHTS)
        logger.info("Loaded %d seeded weights from %s", valid, path)
        return result
    except FileNotFoundError:
        logger.warning("Weight file %s not found, using hardcoded defaults", path)
        return dict(_FALLBACK_WEIGHTS)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        logger.warning("Corrupt weight file %s: %s, using hardcoded defaults", path, exc)
        return dict(_FALLBACK_WEIGHTS)
    except OSError as exc:
        logger.warning("Cannot read weight file %s: %s, using hardcoded defaults", path, exc)
        return dict(_FALLBACK_WEIGHTS)
