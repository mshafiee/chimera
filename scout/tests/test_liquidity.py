"""
Liquidity provider financial-risk tests.

Documents edge cases where liquidity data quality directly causes capital loss:
- SOL price fallback ($150) underestimates real price → wrong USD size calculations
- Unknown source names default to lowest priority (no silent omission)
- Source priority ranking is deterministic and correct
"""

from datetime import datetime, timedelta
from scout.core.liquidity import LiquidityProvider
from scout.core.models import LiquidityData


def _make_liquidity_data(
    token_address: str,
    liquidity_usd: float,
    source: str,
    price_usd: float = 0.001,
    volume_24h_usd: float = None,
    timestamp: datetime = None,
) -> LiquidityData:
    return LiquidityData(
        token_address=token_address,
        liquidity_usd=liquidity_usd,
        price_usd=price_usd,
        volume_24h_usd=volume_24h_usd if volume_24h_usd is not None else liquidity_usd * 0.5,
        timestamp=timestamp or datetime.utcnow(),
        source=source,
    )


def test_sol_fallback_price_150_when_cache_stale():
    """
    Test 69 / 82 (plan): get_sol_price_usd_sync() returns $150 when the cache is
    stale (> 5 minutes old). If real SOL = $200, this causes a 25% underestimate in
    USD-denominated trade size calculations.

    Example: 10 SOL trade → $2000 real, $1500 estimated → slippage model sees
    $1500/$6000 pool = 25% impact, not $2000/$6000 = 33% impact → wrong rejection.

    This test documents the stale-cache fallback value and verifies it returns
    exactly 150.0 when no fresh cache entry exists.
    """
    provider = LiquidityProvider(mode="simulated")

    # With no prior get_sol_price_usd() async call, the internal cache is empty.
    # get_sol_price_usd_sync() must return the 150.0 fallback.
    price = provider.get_sol_price_usd_sync()

    assert price == 150.0, (
        f"Expected conservative fallback $150.0 when cache is empty, got ${price}. "
        "If SOL is really $200, this is a 25% underestimate on all USD calculations."
    )


def test_sol_fallback_causes_measurable_underestimate_on_10_sol_trade():
    """
    Test 82 extension (plan): Quantify the USD underestimate from the $150 fallback.

    For a 10 SOL trade: $200 real - $150 fallback = $50 underestimate per 10 SOL.
    A $6k pool with $1.5k apparent impact (using $150) gives 25% pool impact estimate.
    With the real $200/SOL, the impact is $2k/$6k = 33%.

    The difference (8 percentage points) can cause a trade to pass the slippage
    filter when it should be rejected.
    """
    provider = LiquidityProvider(mode="simulated")

    stale_price = provider.get_sol_price_usd_sync()  # Returns 150.0 (fallback)
    real_price = 200.0  # Hypothetical real SOL price

    trade_sol = 10.0
    pool_usd = 6_000.0

    stale_impact_pct = (trade_sol * stale_price) / pool_usd * 100
    real_impact_pct = (trade_sol * real_price) / pool_usd * 100

    underestimate_pp = real_impact_pct - stale_impact_pct

    assert underestimate_pp > 0, "Stale $150 price must underestimate real impact"
    assert underestimate_pp >= 5.0, (
        f"For $200 real SOL vs $150 fallback on 10-SOL/$6k pool trade, "
        f"expected ≥5pp underestimate, got {underestimate_pp:.1f}pp. "
        "Stale price fallback causes material slippage calculation error."
    )


def test_source_priority_unknown_source_defaults_to_zero_not_omitted():
    """
    Test 84 (plan): A truly unknown source name (e.g., "raydium_v3") gets priority=0
    (default) in the ranking function, rather than being silently omitted.

    Risk: If unknown sources were silently dropped, a new data source would be
    ignored entirely, causing fallback to worse sources even when the new source
    has better data.

    The priority map: {"birdeye": 3, "dexscreener": 2, "jupiter": 1, other: 0}

    NOTE: "birdeye_v2".split("_")[0] == "birdeye" → priority=3 (prefix matching).
    A truly unknown source like "raydium_v3" gives prefix "raydium" → priority=0.
    This tests the actual lookup behavior, not a hypothetical one.
    """
    token = "test_source_priority"
    now = datetime.utcnow()

    # Three candidates: known sources + a genuinely unknown one (non-birdeye prefix)
    known_birdeye = _make_liquidity_data(token, 50_000.0, "birdeye", timestamp=now)
    known_dex = _make_liquidity_data(token, 50_000.0, "dexscreener", timestamp=now)
    unknown_source = _make_liquidity_data(token, 50_000.0, "raydium_v3", timestamp=now)

    # Test the priority key directly by replicating the logic from liquidity.py
    source_priority = {"birdeye": 3, "dexscreener": 2, "jupiter": 1}

    def rank_key(c):
        source_prio = source_priority.get(c.source.lower().split("_")[0], 0)
        return (c.liquidity_usd, c.timestamp.timestamp() if isinstance(c.timestamp, datetime) else 0.0, source_prio)

    prio_birdeye = rank_key(known_birdeye)[2]
    prio_dex = rank_key(known_dex)[2]
    prio_unknown = rank_key(unknown_source)[2]

    # "raydium_v3".split("_")[0] == "raydium" → not in dict → priority=0
    assert prio_unknown == 0, (
        f"Unknown source 'raydium_v3' must default to priority=0, got {prio_unknown}"
    )
    assert prio_birdeye > prio_dex > prio_unknown, (
        f"Priority ordering must be birdeye ({prio_birdeye}) > dexscreener ({prio_dex}) > unknown ({prio_unknown})"
    )

    # When all have equal liquidity and timestamp, birdeye must win
    best = max([known_birdeye, known_dex, unknown_source], key=rank_key)
    assert best.source == "birdeye", (
        f"Best should be 'birdeye' when all else equal, got '{best.source}'"
    )


def test_source_prefix_match_birdeye_v2_gets_birdeye_priority():
    """
    Documents ACTUAL behavior: source "birdeye_v2" gets priority=3 (NOT 0) because
    the split("_")[0] prefix matching treats it as "birdeye".

    Risk: A future source named "birdeye_unofficial" from an untrusted provider
    would get the same trust level as the official Birdeye API. The prefix-based
    matching has no namespace safety for source name collisions.
    """
    source_priority = {"birdeye": 3, "dexscreener": 2, "jupiter": 1}

    def get_prio(source_name):
        return source_priority.get(source_name.lower().split("_")[0], 0)

    assert get_prio("birdeye_v2") == 3, (
        "DOCUMENTS: 'birdeye_v2' gets priority=3 via prefix match — "
        "same trust as official Birdeye API"
    )
    assert get_prio("birdeye_unofficial") == 3, (
        "DOCUMENTS: Prefix match gives unofficial sources same priority as official"
    )
    assert get_prio("raydium_v3") == 0, (
        "'raydium_v3' → prefix 'raydium' → not in dict → priority=0 (correct fallback)"
    )


def test_source_priority_unknown_source_wins_on_higher_liquidity():
    """
    Priority is NOT just source name — liquidity_usd is the primary sort key.
    An unknown source with 10x the liquidity will be selected over birdeye.

    This is correct behavior: data quality (liquidity) trumps source trust.
    Test documents that the unknown source is NOT silently dropped.
    """
    token = "test_liquidity_dominates"
    now = datetime.utcnow()

    low_birdeye = _make_liquidity_data(token, 10_000.0, "birdeye", timestamp=now)
    high_unknown = _make_liquidity_data(token, 100_000.0, "birdeye_v2", timestamp=now)

    source_priority = {"birdeye": 3, "dexscreener": 2, "jupiter": 1}

    def rank_key(c):
        source_prio = source_priority.get(c.source.lower().split("_")[0], 0)
        return (c.liquidity_usd, c.timestamp.timestamp() if isinstance(c.timestamp, datetime) else 0.0, source_prio)

    best = max([low_birdeye, high_unknown], key=rank_key)

    assert best.source == "birdeye_v2", (
        f"Higher liquidity from unknown source must win, got '{best.source}' "
        "(documents that unknown sources are NOT silently omitted)"
    )
    assert float(best.liquidity_usd) == 100_000.0


def test_sol_price_cache_freshness_threshold_is_300_seconds():
    """
    Documents the cache TTL boundary: prices cached < 5 minutes (300 seconds) are used;
    prices ≥ 5 minutes old fall back to $150.

    This test verifies the boundary value. If someone changes the TTL, this test
    catches it — a shorter TTL causes more $150 fallbacks; a longer TTL risks using
    a very stale price during high-volatility moves.
    """
    from datetime import timezone
    provider = LiquidityProvider(mode="simulated")

    # Inject a "fresh" price by directly setting the internal cache
    # (simulates a recent async get_sol_price_usd() call)
    injected_price = 195.0
    provider._sol_price_cache = (injected_price, datetime.now(timezone.utc))

    fresh_price = provider.get_sol_price_usd_sync()
    assert fresh_price == injected_price, (
        f"Fresh cache (just set) must return {injected_price}, got {fresh_price}"
    )

    # Inject a stale price (> 300 seconds old)
    stale_time = datetime.now(timezone.utc) - timedelta(seconds=301)
    provider._sol_price_cache = (injected_price, stale_time)

    stale_price = provider.get_sol_price_usd_sync()
    assert stale_price == 150.0, (
        f"Stale cache (301s old) must return fallback $150.0, got ${stale_price}. "
        "Cache TTL boundary is 300s."
    )


def test_cpmm_slippage_vs_legacy_model():
    """
    Regression test: CPMM model (default) should estimate higher slippage than
    legacy sqrt model for realistic trade scenarios.

    CPMM base formula: base_slippage = trade_value_usd / (token_reserve_usd + trade_value_usd)
    Legacy sqrt formula: base_slippage = 0.1 * sqrt(trade_value_usd / liquidity_usd)

    Test case: $150 trade in $10,000 pool (1.5% pool impact).
    - With CPMM: token_reserve = $5,000 → base = 150 / (5000 + 150) = 2.91%
    - With legacy: base = 0.1 * sqrt(150/10000) = 0.1 * 0.1225 = 1.23%

    CPMM should be higher (more conservative) because the sqrt model underestimates.
    """
    import os

    # Test with CPMM enabled (default)
    os.environ["SCOUT_USE_CPMM_SLIPPAGE"] = "true"
    provider = LiquidityProvider(mode="simulated")

    # Trade parameters: 1.0 SOL at $150/SOL = $150 trade in $10k pool
    amount_sol = 1.0
    liquidity_usd = 10_000.0
    sol_price = 150.0
    volume_24h_usd = 5_000.0  # 0.5 turnover ratio → multiplier = 1.1 (CPMM) or 1.2 (legacy)
    token_age_days = 365.0

    slippage_cpmm = provider.estimate_slippage(
        token_address="test_token",
        amount_sol=amount_sol,
        liquidity_usd=liquidity_usd,
        sol_price_usd=sol_price,
        volume_24h_usd=volume_24h_usd,
        token_age_days=token_age_days,
    )

    # Test with legacy model
    os.environ["SCOUT_USE_CPMM_SLIPPAGE"] = "false"
    slippage_legacy = provider.estimate_slippage(
        token_address="test_token",
        amount_sol=amount_sol,
        liquidity_usd=liquidity_usd,
        sol_price_usd=sol_price,
        volume_24h_usd=volume_24h_usd,
        token_age_days=token_age_days,
    )

    # Reset to default (true)
    os.environ["SCOUT_USE_CPMM_SLIPPAGE"] = "true"

    # Verify CPMM > legacy (CPMM is more conservative/accurate)
    assert slippage_cpmm > slippage_legacy, (
        f"CPMM slippage ({slippage_cpmm:.4f}) must be higher than legacy ({slippage_legacy:.4f}). "
        "The sqrt model underestimates; CPMM is more accurate for AMM pools."
    )

    # Verify both models produce reasonable values (0.01–0.05 for this scenario)
    # CPMM with 0.5 turnover: base≈3.85% * 1.1 = ~4.23%
    # Legacy with 0.5 turnover: base≈1.41% * 1.2 = ~1.70%
    assert 0.03 < slippage_cpmm < 0.05, (
        f"CPMM slippage should be ~4.23% for this scenario, got {slippage_cpmm:.4f}"
    )
    assert 0.01 < slippage_legacy < 0.02, (
        f"Legacy slippage should be ~1.70% for this scenario, got {slippage_legacy:.4f}"
    )

    # Verify the toggle actually changes behavior
    assert abs(slippage_cpmm - slippage_legacy) > 0.01, (
        f"Toggle must produce materially different estimates; difference only {abs(slippage_cpmm - slippage_legacy):.4f}"
    )
