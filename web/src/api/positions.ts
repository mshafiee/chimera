import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'
import type { Position } from '../types'

interface PositionsResponse {
  positions: Position[]
  total: number
}

export function usePositions(state?: string) {
  return useQuery({
    queryKey: ['positions', state],
    queryFn: async () => {
      const params = new URLSearchParams()
      if (state) params.set('state', state)
      
      const { data } = await apiClient.get<PositionsResponse>('/positions', { params })
      return data
    },
    refetchInterval: 10000, // Poll every 10 seconds
  })
}

export function usePosition(tradeUuid: string) {
  return useQuery({
    queryKey: ['position', tradeUuid],
    queryFn: async () => {
      const { data } = await apiClient.get<Position>(`/positions/${tradeUuid}`)
      return data
    },
    enabled: !!tradeUuid,
  })
}
