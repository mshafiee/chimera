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

// Mock data for when API is not available
const mockMarketRegime: MarketRegimeResponse = {
  current_regime: 'neutral',
  confidence: 0,
  volatility_index: 0,
  trend_strength: 0,
  last_regime_change: new Date().toISOString(),
  regime_history: [],
  performance_by_regime: []
}

const mockMarketConditions: MarketConditionsResponse = {
  volatility_index: 0,
  trend_strength: 0,
  liquidity_index: 0,
  market_sentiment: 'neutral',
  risk_level: 'low',
  recommended_allocation: {
    shield_percent: 70,
    spear_percent: 30
  }
}

// Fetch Market Regime
export function useMarketRegime() {
  return useQuery({
    queryKey: ['market', 'regime'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<MarketRegimeResponse>('/market/regime')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Market API] Regime endpoint not implemented, using mock data')
          return mockMarketRegime
        }
        throw error
      }
    },
    refetchInterval: 60000,
    staleTime: 30000,
  })
}

// Fetch Market Conditions
export function useMarketConditions() {
  return useQuery({
    queryKey: ['market', 'conditions'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<MarketConditionsResponse>('/market/conditions')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Market API] Conditions endpoint not implemented, using mock data')
          return mockMarketConditions
        }
        throw error
      }
    },
    refetchInterval: 30000,
    staleTime: 15000,
  })
}
