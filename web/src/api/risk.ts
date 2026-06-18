import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Portfolio Risk Response
export interface PortfolioRiskResponse {
  portfolio_heat_percent: number
  heat_threshold: number
  heat_status: 'normal' | 'elevated' | 'high' | 'critical'
  concentration: ConcentrationData
  exposure: ExposureData
  drawdown: DrawdownData
}

export interface ConcentrationData {
  by_token: TokenConcentration[]
  by_sector: SectorConcentration[]
  max_concentration_percent: number
  hhi: number // Herfindahl-Hirschman Index
}

export interface TokenConcentration {
  token_address: string
  token_symbol: string | null
  position_count: number
  total_value_sol: number
  percentage: number
}

export interface SectorConcentration {
  sector: string
  position_count: number
  total_value_sol: number
  percentage: number
}

export interface ExposureData {
  total_exposure_sol: number
  long_exposure_sol: number
  short_exposure_sol: number
  net_exposure_sol: number
  max_drawdown_percent: number
  current_drawdown_percent: number
}

export interface DrawdownData {
  current_drawdown_percent: number
  max_drawdown_percent: number
  drawdown_duration_days: number
  recovery_percent: number
}

// Stop Loss Metrics
export interface StopLossMetricsResponse {
  activation_rate: number
  total_activations: number
  loss_prevented_sol: number
  average_loss_prevented_sol: number
  activations_by_strategy: StrategyStopLossData[]
  recent_activations: StopLossActivation[]
}

export interface StrategyStopLossData {
  strategy: 'SHIELD' | 'SPEAR'
  activations: number
  loss_prevented_sol: number
}

export interface StopLossActivation {
  timestamp: string
  trade_uuid: string
  token_symbol: string | null
  entry_price: number
  stop_price: number
  loss_prevented_sol: number
  strategy: 'SHIELD' | 'SPEAR'
}

// Profit Target Metrics
export interface ProfitTargetMetricsResponse {
  hit_rate: number
  total_hits: number
  total_targets: number
  trailing_stop_activations: number
  average_realized_gain_sol: number
  targets_by_strategy: StrategyProfitTargetData[]
  recent_hits: ProfitTargetHit[]
}

export interface StrategyProfitTargetData {
  strategy: 'SHIELD' | 'SPEAR'
  hit_rate: number
  total_hits: number
  average_gain_sol: number
}

export interface ProfitTargetHit {
  timestamp: string
  trade_uuid: string
  token_symbol: string | null
  target_level: number
  realized_gain_sol: number
  strategy: 'SHIELD' | 'SPEAR'
}

// Position Size Analysis
export interface PositionSizeAnalysisResponse {
  average_position_sol: number
  median_position_sol: number
  max_position_sol: number
  min_position_sol: number
  position_size_distribution: SizeBucket[]
  kelly_criterion_usage: number
}

export interface SizeBucket {
  range: string
  count: number
  percentage: number
}

// Fetch Portfolio Risk
export function usePortfolioRisk() {
  return useQuery({
    queryKey: ['risk', 'portfolio'],
    queryFn: async () => {
      const response = await apiClient.get<PortfolioRiskResponse>('/api/v1/risk/portfolio')
      return response.data
    },
    refetchInterval: 15000,
    staleTime: 5000,
  })
}

// Fetch Stop Loss Metrics
export function useStopLossMetrics(timeRange?: string) {
  return useQuery({
    queryKey: ['risk', 'stop-loss', timeRange],
    queryFn: async () => {
      const response = await apiClient.get<StopLossMetricsResponse>('/api/v1/risk/stop-loss', {
        params: timeRange ? { range: timeRange } : undefined,
      })
      return response.data
    },
    staleTime: 60000,
  })
}

// Fetch Profit Target Metrics
export function useProfitTargetMetrics(timeRange?: string) {
  return useQuery({
    queryKey: ['risk', 'profit-targets', timeRange],
    queryFn: async () => {
      const response = await apiClient.get<ProfitTargetMetricsResponse>('/api/v1/risk/profit-targets', {
        params: timeRange ? { range: timeRange } : undefined,
      })
      return response.data
    },
    staleTime: 60000,
  })
}

// Fetch Position Size Analysis
export function usePositionSizeAnalysis() {
  return useQuery({
    queryKey: ['risk', 'position-size'],
    queryFn: async () => {
      const response = await apiClient.get<PositionSizeAnalysisResponse>('/api/v1/risk/position-size')
      return response.data
    },
    staleTime: 300000, // 5 minutes
  })
}
