import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiClient } from './client'
import type { Wallet } from '../types'

interface WalletsResponse {
  wallets: Wallet[]
  total: number
}

export function useWallets(status?: string) {
  return useQuery({
    queryKey: ['wallets', status],
    queryFn: async () => {
      const params = new URLSearchParams()
      if (status) params.set('status', status)
      
      const { data } = await apiClient.get<WalletsResponse>('/wallets', { params })
      return data
    },
  })
}

export function useWallet(address: string) {
  return useQuery({
    queryKey: ['wallet', address],
    queryFn: async () => {
      const { data } = await apiClient.get<Wallet>(`/wallets/${address}`)
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
      const { data } = await apiClient.put<UpdateWalletResponse>(`/wallets/${address}`, body)
      return data
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['wallets'] })
    },
  })
}
