import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiClient } from './client'
import type { Wallet } from '../types'

interface WalletsResponse {
  wallets: Wallet[]
  total: number
}

const WALLETS_ENDPOINT = '/wallets'

export function useWallets(status?: string) {
  return useQuery({
    queryKey: status ? ['wallets', status] : ['wallets'],
    queryFn: async ({ signal }) => {
      const params = new URLSearchParams()
      if (status) params.set('status', status)

      const { data } = await apiClient.get<WalletsResponse>(WALLETS_ENDPOINT, { params, signal })
      return data
    },
  })
}

export function useWallet(address: string) {
  return useQuery({
    queryKey: ['wallet', address],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<Wallet>(`${WALLETS_ENDPOINT}/${encodeURIComponent(address)}`, { signal })
      return data
    },
    enabled: !!address,
  })
}

interface UpdateWalletRequest {
  status: 'ACTIVE' | 'CANDIDATE' | 'REJECTED'
  reason?: string
  ttl_hours?: number
}

interface UpdateWalletResponse {
  success: boolean
  wallet: Wallet | null
  message: string
}

export function useUpdateWallet() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ address, ...body }: UpdateWalletRequest & { address: string }) => {
      const { data } = await apiClient.put<UpdateWalletResponse>(`${WALLETS_ENDPOINT}/${encodeURIComponent(address)}`, body)
      if (!data.success) {
        throw new Error(data.message || 'Failed to update wallet')
      }
      return data
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['wallets'] })
      queryClient.invalidateQueries({ queryKey: ['wallet'] })
    },
  })
}
