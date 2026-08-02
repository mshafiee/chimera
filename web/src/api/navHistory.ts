import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

/**
 * Mark-to-market NAV (equity-curve) history.
 *
 * Source: operator `GET /api/v1/portfolio/nav-history`, backed by the
 * `portfolio_snapshots` table written every ~60s. NAV reflects paper
 * performance: `nav_sol = capital_sol + realized_pnl_sol + unrealized_pnl_sol`.
 */

export interface NavHistoryPoint {
  /** ISO-8601 timestamp. */
  recorded_at: string
  /** Mark-to-market NAV (SOL). */
  nav_sol: number
  /** Configured total capital at snapshot time (SOL). */
  capital_sol: number
  /** Cumulative realized PnL from CLOSED positions (SOL). */
  realized_pnl_sol: number
  /** Unrealized PnL of ACTIVE positions at snapshot time (SOL). */
  unrealized_pnl_sol: number
  /** Open (ACTIVE/EXITING) position count at snapshot time. */
  open_positions: number
}

export interface NavHistoryResponse {
  points: NavHistoryPoint[]
  latest_nav_sol: number | null
  latest_unrealized_pnl_sol: number | null
}

export function useNavHistory(days = 30) {
  return useQuery({
    queryKey: ['nav-history', days],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<NavHistoryResponse>('/portfolio/nav-history', {
        params: { days },
        signal,
      })
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 10000,
  })
}
