import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Market Regime Types
export type MarketRegime = 'bull' | 'bear' | 'neutral' | 'volatile'

// Market Regime Response
export interface MarketRegimeResponse {
  current_regime: MarketRegime
  confidence: number
  volatility_index: number
  trend_strength: number
  last_regime_change: string
  regime_history: RegimeHistoryPoint[]
  performance_by_regime: PerformanceByRegime[]
}

export interface RegimeHistoryPoint {
  timestamp: string
  regime: MarketRegime
  volatility_index: number
}

export interface PerformanceByRegime {
  regime: MarketRegime
  total_trades: number
  win_rate: number
  avg_return: number
  total_pnl: number
  sharpe_ratio: number
}

// Market Conditions
export interface MarketConditionsResponse {
  volatility_index: number
  trend_strength: number
  liquidity_index: number
  market_sentiment: 'bullish' | 'bearish' | 'neutral'
  risk_level: 'low' | 'medium' | 'high'
  recommended_allocation: {
    shield_percent: number
    spear_percent: number
  }
}

// Fetch Market Regime
export function useMarketRegime() {
  return useQuery({
    queryKey: ['market', 'regime'],
    queryFn: async () => {
      const response = await apiClient.get<MarketRegimeResponse>('/market/regime')
      return response.data
    },
    refetchInterval: 60000,
    staleTime: 30000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Market API] Failed to fetch market regime:', error)
        // Market regime is optional - console only
      },
    },
  })
}

// Fetch Market Conditions
export function useMarketConditions() {
  return useQuery({
    queryKey: ['market', 'conditions'],
    queryFn: async () => {
      const response = await apiClient.get<MarketConditionsResponse>('/market/conditions')
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 15000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Market API] Failed to fetch market conditions:', error)
        // Market conditions are optional - console only
      },
    },
  })
}
