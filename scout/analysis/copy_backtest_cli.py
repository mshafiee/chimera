"""CLI: python -m analysis.copy_backtest_cli {exit|gate|strategy|gap} [opts]

Runs the repeatable copy-engine backtest over shadow history (Phase 1).

Commands:
  exit      metric table per exit_strategy (cost-adjusted)
  gate      metric table per entry gate under a given exit strategy
  strategy  metric table split by Shield/Spear
  gap       predicted (shadow mirror_main) vs realized (closed trades) win rate
  skew      realized live-vs-mark sell fill skew + defer-trigger bands

Options:
  --cost X      override cost-per-SOL (default: observed from trades)
  --exit STRAT  exit strategy for gate/strategy reports (default mirror_main)
"""

import sys
from decimal import Decimal

from core.copy_backtest import CopyBacktest, format_report

_HELP = "usage: python -m analysis.copy_backtest_cli {exit|gate|strategy|gap} [--cost X] [--exit STRAT]"


def main(argv: list[str]) -> int:
    if not argv or argv[0] in {"-h", "--help"}:
        print(_HELP, file=sys.stderr)
        return 2

    cmd = argv[0]
    cost: Decimal | None = None
    exit_strat = "mirror_main"
    rest = argv[1:]
    i = 0
    while i < len(rest):
        if rest[i] == "--cost" and i + 1 < len(rest):
            cost = Decimal(rest[i + 1])
            i += 2
        elif rest[i] == "--exit" and i + 1 < len(rest):
            exit_strat = rest[i + 1]
            i += 2
        else:
            print(_HELP, file=sys.stderr)
            return 2

    bt = CopyBacktest(cost_per_sol=cost)
    if cmd == "exit":
        print(format_report("per-exit-strategy (cost-adjusted)", bt.per_exit_strategy()))
    elif cmd == "gate":
        print(format_report(f"per-gate under {exit_strat} (cost-adjusted)", bt.per_gate(exit_strat)))
    elif cmd == "strategy":
        print(format_report(f"by-strategy under {exit_strat} (cost-adjusted)", bt.by_strategy(exit_strat)))
    elif cmd == "gap":
        print(bt.realize_vs_price_gap())
    elif cmd == "skew":
        print(bt.fill_skew_report())
    else:
        print(_HELP, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
