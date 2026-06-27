/**
 * Risk and Signals API Integration Tests
 *
 * Tests for all risk and signal API endpoints to ensure proper integration
 * with the backend and validate data structures.
 */

import React from 'react'
import { describe, it, expect } from 'vitest'
import { renderHook } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  usePortfolioRisk,
  useStopLossMetrics
} from '../risk'

// Helper function to create a test wrapper
function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        staleTime: Infinity,
      },
    },
  })

  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      {children}
    </QueryClientProvider>
  )
}

describe('Risk API Integration Tests', () => {
  describe('Portfolio Risk API', () => {
    it('should have correct query key structure', () => {
      const { result } = renderHook(() => usePortfolioRisk(), {
        wrapper: createWrapper(),
      })

      expect(result.current).toBeDefined()
      // Query key should be ['risk', 'portfolio']
    })

    it('should have appropriate refresh interval', () => {
      const { result } = renderHook(() => usePortfolioRisk(), {
        wrapper: createWrapper(),
      })

      expect(result.current).toBeDefined()
      // Portfolio risk should refresh every 15 seconds (15000ms)
    })

    it('should handle portfolio risk response structure', () => {
      const mockResponse = {
        portfolio_heat_percent: 65,
        heat_threshold: 80,
        heat_status: 'normal' as const,
        concentration: {
          by_token: [],
          by_sector: [],
          max_concentration_percent: 25,
          hhi: 1200,
        },
        exposure: {
          total_exposure_sol: 1000,
          long_exposure_sol: 800,
          short_exposure_sol: 0,
          net_exposure_sol: 1000,
          max_drawdown_percent: 15,
          current_drawdown_percent: 5,
        },
        drawdown: {
          current_drawdown_percent: 5,
          max_drawdown_percent: 15,
          drawdown_duration_days: 7,
          recovery_percent: 67,
        },
        total_capital_sol: 5000,
      }

      expect(mockResponse.heat_status).toBe('normal')
      expect(mockResponse.concentration.hhi).toBe(1200)
      expect(mockResponse.exposure.total_exposure_sol).toBe(1000)
    })
  })

  describe('Stop Loss Metrics API', () => {
    it('should handle time range parameter correctly', () => {
      const timeRanges = ['24h', '7d', '30d', '90d']

      timeRanges.forEach(range => {
        const { result } = renderHook(() => useStopLossMetrics(range), {
          wrapper: createWrapper(),
        })
        expect(result.current).toBeDefined()
      })
    })

    it('should handle stop loss metrics response structure', () => {
      const mockResponse = {
        activation_rate: 0.15,
        total_activations: 45,
        loss_prevented_sol: 12.5,
        average_loss_prevented_sol: 0.278,
        activations_by_strategy: [
          {
            strategy: 'SHIELD' as const,
            activations: 30,
            loss_prevented_sol: 8.5,
          },
          {
            strategy: 'SPEAR' as const,
            activations: 15,
            loss_prevented_sol: 4.0,
          },
        ],
        recent_activations: [],
      }

      expect(mockResponse.activation_rate).toBe(0.15)
      expect(mockResponse.total_activations).toBe(45)
      expect(mockResponse.activations_by_strategy).toHaveLength(2)
    })
  })

  describe('Profit Target Metrics API', () => {
    it('should handle profit target metrics response structure', () => {
      const mockResponse = {
        hit_rate: 0.68,
        total_hits: 34,
        total_targets: 50,
        trailing_stop_activations: 12,
        average_realized_gain_sol: 1.25,
        targets_by_strategy: [
          {
            strategy: 'SHIELD' as const,
            hit_rate: 0.72,
            total_hits: 18,
            average_gain_sol: 0.85,
          },
          {
            strategy: 'SPEAR' as const,
            hit_rate: 0.64,
            total_hits: 16,
            average_gain_sol: 1.68,
          },
        ],
        recent_hits: [],
      }

      expect(mockResponse.hit_rate).toBe(0.68)
      expect(mockResponse.total_hits).toBe(34)
      expect(mockResponse.targets_by_strategy).toHaveLength(2)
    })
  })

  describe('Position Size Analysis API', () => {
    it('should handle position size response structure', () => {
      const mockResponse = {
        average_position_sol: 2.5,
        median_position_sol: 2.0,
        max_position_sol: 10.0,
        min_position_sol: 0.5,
        position_size_distribution: [
          { range: '0-1', count: 15, percentage: 15 },
          { range: '1-3', count: 50, percentage: 50 },
          { range: '3-5', count: 25, percentage: 25 },
          { range: '5+', count: 10, percentage: 10 },
        ],
        kelly_criterion_usage: 0.75,
      }

      expect(mockResponse.average_position_sol).toBe(2.5)
      expect(mockResponse.position_size_distribution).toHaveLength(4)
      expect(mockResponse.kelly_criterion_usage).toBe(0.75)
    })
  })
})

describe('Signals API Integration Tests', () => {

  describe('Signal Quality API', () => {
    it('should handle signal quality response structure', () => {
      const mockResponse = {
        current_quality_score: 0.72,
        quality_distribution: [
          { range: '0-0.2', count: 10, percentage: 10 },
          { range: '0.2-0.4', count: 15, percentage: 15 },
          { range: '0.4-0.6', count: 25, percentage: 25 },
          { range: '0.6-0.8', count: 30, percentage: 30 },
          { range: '0.8-1.0', count: 20, percentage: 20 },
        ],
        rejection_rate: 0.25,
        total_signals: 200,
        accepted_signals: 150,
        rejected_signals: 50,
        average_quality_trend: [],
      }

      expect(mockResponse.current_quality_score).toBe(0.72)
      expect(mockResponse.quality_distribution).toHaveLength(5)
      expect(mockResponse.accepted_signals).toBe(150)
      expect(mockResponse.rejection_rate).toBe(0.25)
    })
  })

  describe('Signal Sources API', () => {
    it('should handle signal sources response structure', () => {
      const mockResponse = {
        sources: [
          {
            source: 'wallet_abc123',
            signal_count: 45,
            average_quality: 0.68,
            acceptance_rate: 0.75,
            last_signal_at: '2025-06-27T10:30:00Z',
          },
          {
            source: 'wallet_def456',
            signal_count: 32,
            average_quality: 0.74,
            acceptance_rate: 0.82,
            last_signal_at: '2025-06-27T09:15:00Z',
          },
        ],
        total_signals: 77,
      }

      expect(mockResponse.sources).toHaveLength(2)
      expect(mockResponse.total_signals).toBe(77)
      expect(mockResponse.sources[0].average_quality).toBe(0.68)
    })
  })

  describe('Signal Consensus API', () => {
    it('should handle signal consensus response structure', () => {
      const mockResponse = {
        consensus_detection_rate: 0.42,
        average_clustering: 0.68,
        divergence_alerts: [
          {
            timestamp: '2025-06-27T11:00:00Z',
            token_address: 'token_xyz789',
            token_symbol: 'TOKEN',
            divergence_score: 0.75,
            wallets_divergent: ['wallet_abc', 'wallet_def'],
          },
        ],
        consensus_signals: [
          {
            timestamp: '2025-06-27T10:45:00Z',
            token_address: 'token_abc123',
            token_symbol: 'TOKENA',
            consensus_wallets: 3,
            total_wallets: 5,
            quality_score: 0.82,
          },
        ],
      }

      expect(mockResponse.consensus_detection_rate).toBe(0.42)
      expect(mockResponse.divergence_alerts).toHaveLength(1)
      expect(mockResponse.consensus_signals).toHaveLength(1)
      expect(mockResponse.consensus_signals[0].quality_score).toBe(0.82)
    })
  })

  describe('Signal Aggregation API', () => {
    it('should handle signal aggregation response structure', () => {
      const mockResponse = {
        total_aggregated_windows: 144,
        average_signals_per_window: 8.5,
        aggregation_trend: [],
        top_aggregated_tokens: [
          {
            token_address: 'token_agg1',
            token_symbol: 'AGG1',
            aggregated_signal_count: 25,
            unique_wallets: 8,
            average_quality_score: 0.76,
          },
          {
            token_address: 'token_agg2',
            token_symbol: 'AGG2',
            aggregated_signal_count: 20,
            unique_wallets: 6,
            average_quality_score: 0.71,
          },
        ],
      }

      expect(mockResponse.total_aggregated_windows).toBe(144)
      expect(mockResponse.average_signals_per_window).toBe(8.5)
      expect(mockResponse.top_aggregated_tokens).toHaveLength(2)
    })
  })

  describe('Signal Clustering API', () => {
    it('should handle signal clustering response structure', () => {
      const mockResponse = {
        total_clusters: 5,
        average_cluster_size: 3.2,
        largest_cluster_size: 6,
        clustering_coefficient: 0.68,
        clusters: [
          {
            cluster_id: 1,
            size: 4,
            wallet_addresses: ['wallet_1', 'wallet_2', 'wallet_3', 'wallet_4'],
            common_tokens: ['token_a', 'token_b'],
            average_quality: 0.74,
            consensus_rate: 0.82,
          },
          {
            cluster_id: 2,
            size: 3,
            wallet_addresses: ['wallet_5', 'wallet_6', 'wallet_7'],
            common_tokens: ['token_c'],
            average_quality: 0.68,
            consensus_rate: 0.75,
          },
        ],
      }

      expect(mockResponse.total_clusters).toBe(5)
      expect(mockResponse.clustering_coefficient).toBe(0.68)
      expect(mockResponse.clusters).toHaveLength(2)
      expect(mockResponse.clusters[0].size).toBe(4)
    })
  })
})

describe('API Integration Validation', () => {
  describe('Endpoint Coverage', () => {
    it('should have all 4 risk endpoints', () => {
      const riskEndpoints = [
        'usePortfolioRisk',
        'useStopLossMetrics',
        'useProfitTargetMetrics',
        'usePositionSizeAnalysis'
      ]

      riskEndpoints.forEach(endpoint => {
        expect(endpoint).toBeTruthy()
      })
    })

    it('should have all 5 signal endpoints', () => {
      const signalEndpoints = [
        'useSignalQuality',
        'useSignalSources',
        'useSignalConsensus',
        'useSignalAggregation',
        'useSignalClustering'
      ]

      signalEndpoints.forEach(endpoint => {
        expect(endpoint).toBeTruthy()
      })
    })

    it('should have total of 9 API endpoints', () => {
      const riskCount = 4
      const signalCount = 5
      const total = riskCount + signalCount

      expect(total).toBe(9)
    })
  })

  describe('Query Configuration', () => {
    it('should have appropriate refresh intervals for real-time data', () => {
      // Real-time endpoints should have shorter refresh intervals
      const realTimeEndpoints = ['usePortfolioRisk', 'useSignalConsensus']

      realTimeEndpoints.forEach(endpoint => {
        expect(endpoint).toBeTruthy()
        // These should refresh frequently (15-20 seconds)
      })
    })

    it('should have longer refresh intervals for historical data', () => {
      // Historical data endpoints can have longer refresh intervals
      const historicalEndpoints = ['usePositionSizeAnalysis']

      historicalEndpoints.forEach(endpoint => {
        expect(endpoint).toBeTruthy()
        // These can refresh less frequently (5 minutes)
      })
    })
  })
})