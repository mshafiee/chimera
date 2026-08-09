import React from 'react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { Config } from '../Config'

function findButton(text: string): HTMLElement {
  const matches = screen.getAllByRole('button').filter((b) => b.textContent?.includes(text))
  if (matches.length === 0) throw new Error(`Button containing "${text}" not found`)
  return matches[matches.length - 1]
}

function openSection(title: string) {
  fireEvent.click(screen.getByText(title))
}

function renderConfig() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <Config />
    </QueryClientProvider>
  )
}

const apiMock = vi.hoisted(() => ({
  useConfig: vi.fn(),
  useUpdateConfig: vi.fn(),
  useResetCircuitBreaker: vi.fn(),
  useHealth: vi.fn(),
  useConfigAudit: vi.fn(),
}))

const configApiMock = vi.hoisted(() => ({ useTripCircuitBreaker: vi.fn() }))

vi.mock('../../api', () => apiMock)
vi.mock('../../api/config', () => configApiMock)

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('../../components/ui/Toast', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../components/ui/Toast')>()
  return { ...actual, toast: toastMock }
})

import { useAuthStore } from '../../stores/authStore'

const configData = {
  circuit_breakers: { max_loss_24h: 500, max_consecutive_losses: 3, max_drawdown_percent: 15, cool_down_minutes: 30 },
  strategy_allocation: { shield_percent: 70, spear_percent: 30 },
  strategy: { max_position_sol: 2, min_position_sol: 0.01 },
  jito_tip_strategy: { tip_floor: 0.001, tip_ceiling: 0.01, tip_percentile: 50, tip_percent_max: 0.5 },
  jito_enabled: true,
  rpc_status: { primary: 'helius', active: 'jito', fallback_triggered: false },
  monitoring: {
    enabled: true,
    webhook_registration_batch_size: 10,
    webhook_registration_delay_ms: 200,
    webhook_processing_rate_limit: 60,
    rpc_polling_enabled: true,
    rpc_poll_interval_secs: 8,
    rpc_poll_batch_size: 6,
    rpc_poll_rate_limit: 40,
    max_active_wallets: 20,
  },
  profit_management: { targets: [25, 50], tiered_exit_percent: 25, trailing_stop_activation: 50, trailing_stop_distance: 20, hard_stop_loss: 15, time_exit_hours: 24 },
  position_sizing: { base_size_sol: 0.1, max_size_sol: 2, min_size_sol: 0.5, consensus_multiplier: 1.5, max_concurrent_positions: 5 },
  mev_protection: { always_use_jito: true, exit_tip_sol: 0.007, consensus_tip_sol: 0.003, standard_tip_sol: 0.0015 },
  token_safety: { min_liquidity_shield_usd: 10000, min_liquidity_spear_usd: 5000, honeypot_detection_enabled: true, cache_capacity: 1000, cache_ttl_seconds: 3600, freeze_authority_whitelist: ['a'], mint_authority_whitelist: ['b'] },
  notifications: {
    telegram: { enabled: true, rate_limit_seconds: 60 },
    rules: { circuit_breaker_triggered: true, wallet_drained: true, position_exited: true, wallet_promoted: true, daily_summary: true, rpc_fallback: true },
    daily_summary: { enabled: true, hour_utc: 20, minute: 0 },
  },
  queue: { capacity: 1000, load_shed_threshold_percent: 80 },
}

const health = {
  status: 'healthy',
  uptime_seconds: 100,
  queue_depth: 0,
  rpc_latency_ms: 5,
  last_trade_at: null,
  database: { status: 'healthy', message: null },
  rpc: { status: 'healthy', message: null },
  circuit_breaker: { state: 'TRIPPED', trading_allowed: false, trip_reason: 'manual', cooldown_remaining_secs: 60 },
  price_cache: { total_entries: 0, tracked_tokens: 0 },
}

const auditItems = [
  { id: 1, key: 'secret_rotation.webhook', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'rotate', changed_at: '2025-01-01T00:00:00Z' },
  { id: 2, key: 'strategy.max_position_sol', old_value: '1', new_value: '2', changed_by: 'admin', change_reason: null, changed_at: '2025-01-02T00:00:00Z' },
]

function setup() {
  apiMock.useConfig.mockReturnValue({ data: configData, isLoading: false, refetch: vi.fn() })
  apiMock.useHealth.mockReturnValue({ data: health, error: null })
  apiMock.useConfigAudit.mockReturnValue({ data: { items: auditItems, total: 2 }, isLoading: false })
  apiMock.useUpdateConfig.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue(configData),
    isPending: false,
  })
  apiMock.useResetCircuitBreaker.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ success: true }),
    isPending: false,
  })
  configApiMock.useTripCircuitBreaker.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ success: true }),
    isPending: false,
  })
}

function login(role: 'admin' | 'operator' = 'admin') {
  useAuthStore.setState({
    user: { identifier: 'u', role, token: 'tok' },
    isAuthenticated: true,
    tokenExpiresAt: Date.now() + 3600000,
    refreshToken: null,
    lastActivity: Date.now(),
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
  login('admin')
})

describe('Config', () => {
  it('renders the loading state', () => {
    apiMock.useConfig.mockReturnValue({ data: undefined, isLoading: true, refetch: vi.fn() })
    renderConfig()
    expect(screen.getByText('Loading configuration...')).toBeInTheDocument()
  })

  it('renders the full admin configuration', () => {
    renderConfig()
    expect(screen.getByText('Trading Configuration')).toBeInTheDocument()
    expect(screen.getByText('Circuit Breaker Tripped')).toBeInTheDocument()
    expect(screen.getByText('Profit Management')).toBeInTheDocument()
    expect(screen.getByText('Position Sizing')).toBeInTheDocument()
    expect(screen.getByText('MEV Protection')).toBeInTheDocument()
    expect(screen.getByText('Monitoring')).toBeInTheDocument()
    expect(screen.getByText('Token Safety')).toBeInTheDocument()
    expect(screen.getByText('Notifications')).toBeInTheDocument()
    expect(screen.getByText('System Settings')).toBeInTheDocument()
    expect(screen.getByText('Emergency Kill Switch')).toBeInTheDocument()
    expect(screen.getByText('Webhook HMAC Key')).toBeInTheDocument()
  })

  it('saves circuit breaker settings', async () => {
    renderConfig()
    fireEvent.click(screen.getByRole('button', { name: /save circuit breakers/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Circuit breaker settings saved')
    })
  })

  it('saves strategy settings', async () => {
    renderConfig()
    fireEvent.change(document.getElementById('strategy-allocation-slider') as HTMLInputElement, {
      target: { value: '60' },
    })
    fireEvent.click(screen.getByRole('button', { name: /save strategy settings/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Strategy settings saved')
    })
  })

  it('saves profit management settings', async () => {
    renderConfig()
    openSection('Profit Management')
    fireEvent.click(findButton('Save Profit Management'))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Profit management settings saved')
    })
  })

  it('validates position sizes before saving', async () => {
    renderConfig()
    openSection('Position Sizing')
    fireEvent.click(findButton('Save Position Sizing'))
    expect(toastMock.error).toHaveBeenCalledWith('Position sizes must be: min < base < max')
  })

  it('saves position sizing and monitoring settings successfully with valid values', async () => {
    apiMock.useConfig.mockReturnValue({
      data: {
        ...configData,
        position_sizing: { base_size_sol: 0.5, max_size_sol: 2, min_size_sol: 0.05, consensus_multiplier: 1.5, max_concurrent_positions: 5 },
        monitoring: { ...configData.monitoring, webhook_processing_rate_limit: 45, rpc_poll_rate_limit: 40 },
      },
      isLoading: false,
      refetch: vi.fn(),
    })
    renderConfig()
    openSection('Position Sizing')
    openSection('Monitoring')
    fireEvent.click(findButton('Save Position Sizing'))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Position sizing settings saved')
    })
    fireEvent.click(findButton('Save Monitoring Settings'))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Monitoring settings saved')
    })
  })

  it('shows errors when strategy and profit saves fail', async () => {
    apiMock.useUpdateConfig.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('boom')),
      isPending: false,
    })
    renderConfig()
    openSection('Profit Management')
    fireEvent.click(findButton('Save Strategy Settings'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save strategy settings')
    })
    fireEvent.click(findButton('Save Profit Management'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save profit management settings')
    })
  })

  it('shows errors when position sizing and monitoring saves fail', async () => {
    apiMock.useConfig.mockReturnValue({
      data: {
        ...configData,
        position_sizing: { base_size_sol: 0.5, max_size_sol: 2, min_size_sol: 0.05, consensus_multiplier: 1.5, max_concurrent_positions: 5 },
        monitoring: { ...configData.monitoring, webhook_processing_rate_limit: 45, rpc_poll_rate_limit: 40 },
      },
      isLoading: false,
      refetch: vi.fn(),
    })
    apiMock.useUpdateConfig.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('boom')),
      isPending: false,
    })
    renderConfig()
    openSection('Position Sizing')
    openSection('Monitoring')
    fireEvent.click(findButton('Save Position Sizing'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save position sizing settings')
    })
    fireEvent.click(findButton('Save Monitoring Settings'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save monitoring settings')
    })
  })

  it('shows errors when mev, token safety, notifications and queue saves fail', async () => {
    apiMock.useUpdateConfig.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('boom')),
      isPending: false,
    })
    renderConfig()
    openSection('MEV Protection')
    openSection('Token Safety')
    openSection('Notifications')
    fireEvent.click(findButton('Save MEV Protection'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save MEV protection settings')
    })
    fireEvent.click(findButton('Save Token Safety'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save token safety settings')
    })
    fireEvent.click(findButton('Save Notifications'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save notification settings')
    })
    fireEvent.click(findButton('Save Queue Settings'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save queue settings')
    })
  })

  it('shows an error when the circuit breaker reset fails', async () => {
    apiMock.useResetCircuitBreaker.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('no')),
      isPending: false,
    })
    renderConfig()
    fireEvent.click(findButton('Reset Circuit Breaker'))
    fireEvent.click(findButton('Reset & Resume Trading'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to reset circuit breaker')
    })
  })

  it('updates daily summary hour and minute inputs', () => {
    renderConfig()
    openSection('Notifications')
    const hourInputs = document.querySelectorAll('input[type="number"]')
    const hourInput = hourInputs[5] as HTMLInputElement
    const minuteInput = hourInputs[6] as HTMLInputElement
    fireEvent.change(hourInput, { target: { value: '21' } })
    fireEvent.change(minuteInput, { target: { value: '30' } })
    expect(screen.getByText('Hour (UTC)')).toBeInTheDocument()
  })

  it('renders the active circuit breaker badge', () => {
    apiMock.useHealth.mockReturnValue({
      data: { ...health, circuit_breaker: { ...health.circuit_breaker, trading_allowed: true } },
      error: null,
    })
    renderConfig()
    expect(screen.getAllByText('Active').length).toBeGreaterThan(0)
  })

  it('renders rotation history for rpc keys and badge variants', () => {
    const recent = new Date(Date.now() - 3 * 86400 * 1000).toISOString()
    const mid = new Date(Date.now() - 20 * 86400 * 1000).toISOString()
    const old = new Date(Date.now() - 40 * 86400 * 1000).toISOString()
    apiMock.useConfigAudit.mockReturnValue({
      data: {
        items: [
          { id: 1, key: 'secret_rotation.webhook', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'r', changed_at: recent },
          { id: 2, key: 'secret_rotation.webhook', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'r', changed_at: mid },
          { id: 3, key: 'secret_rotation.rpc', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'r', changed_at: recent },
          { id: 4, key: 'secret_rotation.rpc', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'r', changed_at: mid },
          { id: 5, key: 'secret_rotation.webhook', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'r', changed_at: old },
          { id: 6, key: 'secret_rotation.rpc', old_value: null, new_value: 'x', changed_by: 'admin', change_reason: 'r', changed_at: old },
        ],
        total: 4,
      },
      isLoading: false,
    })
    renderConfig()
    expect(screen.getByText('RPC API Keys')).toBeInTheDocument()
  })

  it('renders the audit loading and empty states', () => {
    apiMock.useConfigAudit.mockReturnValue({ data: undefined, isLoading: true })
    renderConfig()
    fireEvent.click(findButton('View History'))
    expect(screen.getByText('Loading history...')).toBeInTheDocument()
  })

  it('renders the empty change history', () => {
    apiMock.useConfigAudit.mockReturnValue({ data: { items: [], total: 0 }, isLoading: false })
    renderConfig()
    fireEvent.click(findButton('View History'))
    expect(screen.getByText('No change history found')).toBeInTheDocument()
  })

  it('cancels the kill switch modal', () => {
    renderConfig()
    fireEvent.click(findButton('Activate Kill Switch'))
    fireEvent.click(findButton('Cancel'))
    expect(screen.queryByText('Type')).not.toBeInTheDocument()
  })

  it('closes the kill switch modal via the close button', () => {
    renderConfig()
    fireEvent.click(findButton('Activate Kill Switch'))
    fireEvent.click(screen.getByLabelText('Close dialog'))
    expect(screen.queryByText('Type')).not.toBeInTheDocument()
  })

  it('validates monitoring rate limits before saving', async () => {
    renderConfig()
    openSection('Monitoring')
    fireEvent.click(findButton('Save Monitoring Settings'))
    expect(toastMock.error).toHaveBeenCalledWith(
      'Rate limits cannot exceed 50 req/sec (Helius limit)'
    )
  })

  it('saves MEV, token safety, notifications and queue settings', async () => {
    renderConfig()
    openSection('MEV Protection')
    openSection('Token Safety')
    openSection('Notifications')
    fireEvent.click(findButton('Save MEV Protection'))
    fireEvent.click(findButton('Save Token Safety'))
    fireEvent.click(findButton('Save Notifications'))
    fireEvent.click(findButton('Save Queue Settings'))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('MEV protection settings saved')
      expect(toastMock.success).toHaveBeenCalledWith('Token safety settings saved')
      expect(toastMock.success).toHaveBeenCalledWith('Notification settings saved')
      expect(toastMock.success).toHaveBeenCalledWith('Queue settings saved')
    })
  })

  it('shows errors when saves fail', async () => {
    apiMock.useUpdateConfig.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('boom')),
      isPending: false,
    })
    renderConfig()
    fireEvent.click(screen.getByRole('button', { name: /save circuit breakers/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to save circuit breaker settings')
    })
  })

  it('resets the circuit breaker via the confirm modal', async () => {
    renderConfig()
    fireEvent.click(findButton('Reset Circuit Breaker'))
    fireEvent.click(screen.getAllByRole('button', { name: /reset & resume trading/i })[0])
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Circuit breaker reset successfully')
    })
  })

  it('shows the config change history modal', () => {
    renderConfig()
    fireEvent.click(screen.getByRole('button', { name: /view history/i }))
    expect(screen.getByText('Configuration Change History')).toBeInTheDocument()
    expect(screen.getByText('secret_rotation.webhook')).toBeInTheDocument()
    expect(screen.getByText('strategy.max_position_sol')).toBeInTheDocument()
    expect(screen.getByText('-')).toBeInTheDocument()
  })

  it('handles the emergency kill switch', async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ success: true })
    configApiMock.useTripCircuitBreaker.mockReturnValue({ mutateAsync, isPending: false })
    renderConfig()
    fireEvent.click(findButton('Activate Kill Switch'))
    expect(screen.getAllByText('Emergency Kill Switch').length).toBeGreaterThan(0)

    // wrong confirmation text
    fireEvent.change(document.getElementById('kill-switch-confirm') as HTMLInputElement, {
      target: { value: 'HAL' },
    })
    expect(screen.getByText('Please type exactly "HALT" to confirm')).toBeInTheDocument()
    fireEvent.change(document.getElementById('kill-switch-confirm') as HTMLInputElement, {
      target: { value: 'HALT' },
    })
    fireEvent.click(findButton('Activate Kill Switch'))
    await waitFor(() => {
      expect(mutateAsync).toHaveBeenCalledWith('Emergency kill switch activated')
    })
    expect(toastMock.success).toHaveBeenCalledWith(
      'Emergency kill switch activated. All trading halted.'
    )
  })

  it('requires authentication before activating the kill switch', async () => {
    useAuthStore.setState({
      user: { identifier: 'u', role: 'admin', token: null },
      isAuthenticated: true,
      tokenExpiresAt: null,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    renderConfig()
    fireEvent.click(findButton('Activate Kill Switch'))
    fireEvent.change(document.getElementById('kill-switch-confirm') as HTMLInputElement, {
      target: { value: 'HALT' },
    })
    fireEvent.click(findButton('Activate Kill Switch'))
    expect(toastMock.error).toHaveBeenCalledWith(
      'You must be authenticated to activate the kill switch. Please log in again.'
    )
  })

  it('shows an error when the kill switch request fails with 401', async () => {
    const errorSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    configApiMock.useTripCircuitBreaker.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue({
        response: { status: 401, data: { reason: 'unauthorized' } },
        message: 'Request failed',
      }),
      isPending: false,
    })
    renderConfig()
    fireEvent.click(findButton('Activate Kill Switch'))
    fireEvent.change(document.getElementById('kill-switch-confirm') as HTMLInputElement, {
      target: { value: 'HALT' },
    })
    fireEvent.click(findButton('Activate Kill Switch'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith(
        'Authentication failed: unauthorized. Please log in again with your admin wallet.'
      )
    })
    errorSpy.mockRestore()
  })

  it('shows a generic error when the kill switch request fails', async () => {
    configApiMock.useTripCircuitBreaker.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue({
        response: { status: 500, data: { details: 'server error' } },
        message: 'Request failed',
      }),
      isPending: false,
    })
    renderConfig()
    fireEvent.click(findButton('Activate Kill Switch'))
    fireEvent.change(document.getElementById('kill-switch-confirm') as HTMLInputElement, {
      target: { value: 'HALT' },
    })
    fireEvent.click(findButton('Activate Kill Switch'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith(
        'Failed to activate kill switch: server error'
      )
    })
  })

  it('renders the read-only view for non-admin users', () => {
    login('operator')
    renderConfig()
    expect(screen.getByText(/Configuration management requires admin access/)).toBeInTheDocument()
    expect(screen.getByText('Circuit Breakers')).toBeInTheDocument()
    expect(screen.getByText('Strategy Allocation')).toBeInTheDocument()
    expect(screen.getByText('Profit Management')).toBeInTheDocument()
    expect(screen.getByText('Monitoring')).toBeInTheDocument()
    expect(screen.getByText('$500')).toBeInTheDocument()
  })

  it('renders rotation history status', () => {
    renderConfig()
    expect(screen.getByText('Webhook HMAC Key')).toBeInTheDocument()
    expect(screen.getByText(/Last Rotated/)).toBeInTheDocument()
  })
})
