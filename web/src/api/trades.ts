import { useQuery } from '@tanstack/react-query'
import { apiClient, getApiError } from './client'
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
  wallet_address?: string
  limit?: number
  offset?: number
}

const TRADES_ENDPOINT = '/trades'
const TRADES_EXPORT_ENDPOINT = '/trades/export'

function buildTradesSearchParams(params: TradesParams): URLSearchParams {
  const searchParams = new URLSearchParams()
  if (params.from) searchParams.set('from', params.from)
  if (params.to) searchParams.set('to', params.to)
  if (params.status) searchParams.set('status', params.status)
  if (params.strategy) searchParams.set('strategy', params.strategy)
  if (params.wallet_address) searchParams.set('wallet_address', params.wallet_address)
  if (params.limit !== undefined) searchParams.set('limit', params.limit.toString())
  if (params.offset !== undefined) searchParams.set('offset', params.offset.toString())
  return searchParams
}

export function useTrades(params: TradesParams = {}) {
  return useQuery({
    queryKey: ['trades', params],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<TradesResponse>(TRADES_ENDPOINT, {
        params: buildTradesSearchParams(params),
        signal,
      })
      return data
    },
  })
}

function parseContentDispositionFilename(header: unknown): string | null {
  if (typeof header !== 'string') return null
  const match = /filename\*?=(?:UTF-8''|")?([^";]+)/i.exec(header)
  if (!match) return null
  const name = match[1].trim().replace(/^"|"$/g, '')
  if (!name || name.includes('/') || name.includes('\\')) return null
  return name
}

export async function exportTrades(
  params: Omit<TradesParams, 'limit' | 'offset'>,
  format: 'csv' | 'json' | 'pdf' = 'csv'
): Promise<void> {
  try {
    const searchParams = buildTradesSearchParams(params)
    searchParams.set('format', format)

    const response = await apiClient.get(TRADES_EXPORT_ENDPOINT, {
      params: searchParams,
      responseType: 'blob',
    })

    // Create download link
    const blob = new Blob([response.data])
    const contentDisposition = response.headers['content-disposition']
    const defaultExtension = format
    const defaultFilename = `chimera_trades_${new Date().toISOString().split('T')[0]}.${defaultExtension}`
    const filename = parseContentDispositionFilename(contentDisposition) ?? defaultFilename

    const url = window.URL.createObjectURL(blob)
    try {
      const a = document.createElement('a')
      a.href = url
      a.download = filename
      a.style.display = 'none'
      document.body.appendChild(a)
      a.click()
      document.body.removeChild(a)
    } finally {
      setTimeout(() => window.URL.revokeObjectURL(url), 0)
    }
  } catch (error) {
    throw new Error(getApiError(error))
  }
}
