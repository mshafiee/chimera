import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiClient } from './client'
import type { ConfigResponse } from '../types'

export function useConfig() {
  return useQuery({
    queryKey: ['config'],
    queryFn: async () => {
      const { data } = await apiClient.get<ConfigResponse>('/config')
      return data
    },
  })
}

interface UpdateConfigRequest {
  circuit_breakers?: {
    max_loss_24h?: number
    max_consecutive_losses?: number
    max_drawdown_percent?: number
    cool_down_minutes?: number
  }
  strategy_allocation?: {
    shield_percent?: number
    spear_percent?: number
  }
}

export function useUpdateConfig() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (body: UpdateConfigRequest) => {
      const { data } = await apiClient.put<ConfigResponse>('/config', body)
      return data
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['config'] })
    },
  })
}

interface CircuitBreakerResetResponse {
  success: boolean
  message: string
  previous_state: string
  new_state: string
}

export function useResetCircuitBreaker() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async () => {
      const { data } = await apiClient.post<CircuitBreakerResetResponse>('/config/circuit-breaker/reset')
      return data
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['config'] })
      queryClient.invalidateQueries({ queryKey: ['health'] })
    },
  })
}
