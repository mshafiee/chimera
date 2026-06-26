import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Wallet Monitoring State Response
export interface WalletMonitoringStateResponse {
  wallet_states: WalletMonitoringStateItem[]
}

export interface WalletMonitoringStateItem {
  address: string
  method: 'webhook' | 'polling'
  status: 'active' | 'inactive' | 'error'
  last_activity: string
  last_fetch: string | null
  failed_fetches: number
  success_rate: number
  next_fetch: string | null
}

// Fetch Wallet Monitoring States
export function useWalletMonitoringStates() {
  return useQuery({
    queryKey: ['wallet-monitoring', 'states'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<WalletMonitoringStateResponse>('/monitoring/wallets/states', { signal })
      return response.data
    },
    refetchInterval: 30000, // Poll every 30 seconds
    staleTime: 10000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Wallet Monitoring API] Failed to fetch states:', error)
        // Wallet monitoring is important - show toast notification
        // Note: Uncomment when toast is available
        // toast.error('Failed to load wallet monitoring states. Please try again later.')
      },
    },
  })
}
