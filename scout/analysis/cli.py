"""CLI: python -m analysis.cli {diagnostic|frontier}"""

import sys

from .db import connect
from .diagnostic import fetch_gate_summary, render_funnel


def main(argv: list[str]) -> int:
    if not argv or argv[0] not in {"diagnostic", "frontier"}:
        print("usage: python -m analysis.cli {diagnostic|frontier}", file=sys.stderr)
        return 2

    conn = connect()
    try:
        if argv[0] == "diagnostic":
            print(render_funnel(fetch_gate_summary(conn)))
            return 0
        # Phase B: lazy import so diagnostic works without the frontier deps.
        from .frontier import run_frontier
        print(run_frontier(conn))
        return 0
    finally:
        conn.close()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
