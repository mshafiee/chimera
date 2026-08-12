"""Profitability evidence-loop analysis.

Read-only tools that turn the shadow trader's counterfactual PnL (recorded for
every signal, admitted or rejected) into:

  * diagnostic — which admission gate rejects the most, and whether it was right
  * frontier   — the achievable (win rate, monthly return, drawdown, trade count)
                 Pareto frontier from loosening each gate

Run from the scout/ directory:  python -m analysis.cli {diagnostic|frontier}
"""
