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
  strategy?: {
    max_position_sol?: number
    min_position_sol?: number
  }
  monitoring?: {
    enabled?: boolean
    webhook_registration_batch_size?: number
    webhook_registration_delay_ms?: number
    webhook_processing_rate_limit?: number
    rpc_polling_enabled?: boolean
    rpc_poll_interval_secs?: number
    rpc_poll_batch_size?: number
    rpc_poll_rate_limit?: number
    max_active_wallets?: number
  }
  profit_management?: {
    targets?: number[]
    tiered_exit_percent?: number
    trailing_stop_activation?: number
    trailing_stop_distance?: number
    hard_stop_loss?: number
    time_exit_hours?: number
  }
  position_sizing?: {
    base_size_sol?: number
    max_size_sol?: number
    min_size_sol?: number
    consensus_multiplier?: number
    max_concurrent_positions?: number
  }
  mev_protection?: {
    always_use_jito?: boolean
    exit_tip_sol?: number
    consensus_tip_sol?: number
    standard_tip_sol?: number
  }
  token_safety?: {
    min_liquidity_shield_usd?: number
    min_liquidity_spear_usd?: number
    honeypot_detection_enabled?: boolean
    cache_capacity?: number
    cache_ttl_seconds?: number
  }
  notifications?: {
    telegram?: {
      enabled?: boolean
      rate_limit_seconds?: number
    }
    rules?: {
      circuit_breaker_triggered?: boolean
      wallet_drained?: boolean
      position_exited?: boolean
      wallet_promoted?: boolean
      daily_summary?: boolean
      rpc_fallback?: boolean
    }
    daily_summary?: {
      enabled?: boolean
      hour_utc?: number
      minute?: number
    }
  }
  queue?: {
    capacity?: number
    load_shed_threshold_percent?: number
  }
  notification_rules?: {
    circuit_breaker_triggered?: boolean
    wallet_drained?: boolean
    position_exited?: boolean
    wallet_promoted?: boolean
    daily_summary?: boolean
    rpc_fallback?: boolean
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
