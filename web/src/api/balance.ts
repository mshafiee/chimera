import { useMemo } from 'react'
import { usePositions, usePortfolioRisk } from './'

/**
 * Hook for calculating Balance and Net Asset Value (NAV)
 *
 * Balance: Current wallet SOL balance from portfolio risk endpoint
 * NAV: Balance + Total Unrealized PnL from all active positions
 */
export function useBalanceAndNAV() {
  const { data: positionsData } = usePositions('ACTIVE')
  const { data: portfolioRisk } = usePortfolioRisk()

  return useMemo(() => {
    // Balance from portfolio risk endpoint (polled every 60s on backend)
    const balance = portfolioRisk?.total_capital_sol ?? 0

    // Calculate NAV: Balance + Total Unrealized PnL
    const totalUnrealizedPnL = positionsData?.total_unrealized_pnl_sol ?? 0
    const nav = balance + totalUnrealizedPnL

    return {
      balance,
      nav,
      totalUnrealizedPnL,
      isLoading: !portfolioRisk || !positionsData,
    }
  }, [portfolioRisk, positionsData])
}
