from datetime import datetime, timedelta
from scout.core.models import WalletRecord, HistoricalTrade, TradeAction
from scout.core.wqs import WalletMetrics, calculate_wqs

def test_ranking():
    # 1. create a "Sniper" metrics object
    # High ROI but buys 15s after launch (Kill Switch Trigger)
    sniper = WalletMetrics(
        address="SniperWallet",
        roi_7d=50.0,
        roi_30d=100.0,
        trade_count_30d=20,
        win_rate=0.8,
        max_drawdown_30d=5.0,
        avg_trade_size_sol=1.0,
        profit_factor=3.0,
        avg_entry_delay_seconds=15.0 # < 30s -> RETURN 0.0 (IMMEDIATE REJECTION)
    )
    
    # 2. create a "Smart Money" metrics object
    # Good ROI but waits 20 mins after launch (Sweet Spot)
    smart_money = WalletMetrics(
        address="SmartMoney",
        roi_7d=30.0,
        roi_30d=60.0,
        trade_count_30d=20,
        win_rate=0.6,
        max_drawdown_30d=8.0,
        avg_trade_size_sol=2.0,
        profit_factor=2.5,
        avg_entry_delay_seconds=1200.0 # 20 mins (within 1h window) -> +15.0
    )
    
    score_sniper = calculate_wqs(sniper)
    score_smart = calculate_wqs(smart_money)
    
    print(f"Sniper Score: {score_sniper}")
    print(f"Smart Money Score: {score_smart}")
    
    # We expect Sniper to be 0.0 and Smart Money to be high
    if score_sniper == 0.0:
        print("PASS: Sniper Killed (0.0).")
    else:
        print(f"FAIL: Sniper survived with score {score_sniper}")
        
    if score_smart > 50.0:
        print("PASS: Smart Money scored high.")
    else:
        print(f"FAIL: Smart Money score too low {score_smart}")

if __name__ == "__main__":
    test_ranking()
