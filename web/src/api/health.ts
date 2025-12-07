import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'
import type { HealthResponse } from '../types'

export function useHealth() {
  return useQuery({
    queryKey: ['health'],
    queryFn: async () => {
      const { data } = await apiClient.get<HealthResponse>('/health')
      return data
    },
    refetchInterval: 5000, // Poll every 5 seconds
  })
}
