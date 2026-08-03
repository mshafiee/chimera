import { useMemo } from 'react'
import { usePositions } from './positions'
import { usePortfolioRisk } from './risk'

/**
 * Hook for calculating Balance and Net Asset Value (NAV)
 *
 * Balance: Current wallet SOL balance from portfolio risk endpoint
 * NAV: Balance + Total Unrealized PnL from all active positions
 */
export function useBalanceAndNAV() {
  const { data: positionsData, isLoading: positionsLoading, isError: positionsError } = usePositions('ACTIVE')
  const { data: portfolioRisk, isLoading: riskLoading, isError: riskError } = usePortfolioRisk()

  return useMemo(() => {
    // Balance from portfolio risk endpoint — actual wallet balance
    // (config capital + realized PnL − active exposure)
    const balance = Number(portfolioRisk?.wallet_balance_sol ?? 0)

    // Calculate NAV: Balance + Total Unrealized PnL
    const totalUnrealizedPnL = Number(positionsData?.total_unrealized_pnl_sol ?? 0)
    const nav = balance + totalUnrealizedPnL

    return {
      balance,
      nav,
      totalUnrealizedPnL,
      isLoading: positionsLoading || riskLoading,
      isError: positionsError || riskError,
    }
  }, [portfolioRisk, positionsData, positionsLoading, riskLoading, positionsError, riskError])
}
