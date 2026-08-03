import { useQuery } from '@tanstack/react-query'
import { apiClient, getApiError } from './client'
import type { Position } from '../types'

interface PositionsResponse {
  positions: Position[]
  total: number
  total_unrealized_pnl_sol: number | null  // Sum of unrealized PnL for all active positions
}

const POSITIONS_ENDPOINT = '/positions'

export function usePositions(state?: string) {
  return useQuery({
    queryKey: ['positions', state],
    queryFn: async ({ signal }) => {
      const params = new URLSearchParams()
      if (state) params.set('state', state)

      const { data } = await apiClient.get<PositionsResponse>(POSITIONS_ENDPOINT, { params, signal })
      return data
    },
    refetchInterval: 10000, // Poll every 10 seconds
    meta: {
      onError: (error: unknown) => {
        console.error('[Positions API] Failed to fetch positions:', getApiError(error))
      },
    },
  })
}

export function usePosition(tradeUuid: string) {
  return useQuery({
    queryKey: ['position', tradeUuid],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<Position>(`${POSITIONS_ENDPOINT}/${encodeURIComponent(tradeUuid)}`, { signal })
      return data
    },
    enabled: !!tradeUuid,
    meta: {
      onError: (error: unknown) => {
        console.error('[Positions API] Failed to fetch position:', getApiError(error))
      },
    },
  })
}
