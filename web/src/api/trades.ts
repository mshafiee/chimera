import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'
import type { Trade } from '../types'

interface TradesResponse {
  trades: Trade[]
  total: number
  limit: number
  offset: number
}

interface TradesParams {
  from?: string
  to?: string
  status?: string
  strategy?: string
  limit?: number
  offset?: number
}

export function useTrades(params: TradesParams = {}) {
  return useQuery({
    queryKey: ['trades', params],
    queryFn: async () => {
      const searchParams = new URLSearchParams()
      if (params.from) searchParams.set('from', params.from)
      if (params.to) searchParams.set('to', params.to)
      if (params.status) searchParams.set('status', params.status)
      if (params.strategy) searchParams.set('strategy', params.strategy)
      if (params.limit) searchParams.set('limit', params.limit.toString())
      if (params.offset) searchParams.set('offset', params.offset.toString())
      
      const { data } = await apiClient.get<TradesResponse>('/trades', { params: searchParams })
      return data
    },
  })
}

export async function exportTrades(params: Omit<TradesParams, 'limit' | 'offset'>): Promise<void> {
  const searchParams = new URLSearchParams()
  if (params.from) searchParams.set('from', params.from)
  if (params.to) searchParams.set('to', params.to)
  if (params.status) searchParams.set('status', params.status)
  if (params.strategy) searchParams.set('strategy', params.strategy)

  const response = await apiClient.get('/trades/export', {
    params: searchParams,
    responseType: 'blob',
  })

  // Create download link
  const url = window.URL.createObjectURL(new Blob([response.data]))
  const link = document.createElement('a')
  link.href = url
  
  // Get filename from Content-Disposition header or use default
  const contentDisposition = response.headers['content-disposition']
  const filename = contentDisposition
    ? contentDisposition.split('filename=')[1]?.replace(/"/g, '')
    : `chimera_trades_${new Date().toISOString().split('T')[0]}.csv`
  
  link.setAttribute('download', filename)
  document.body.appendChild(link)
  link.click()
  link.remove()
  window.URL.revokeObjectURL(url)
}
