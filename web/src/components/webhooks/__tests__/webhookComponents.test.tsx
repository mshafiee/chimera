import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { BulkOperationModal } from '../BulkOperationModal'
import { WebhookActionMenu, WebhookStatusBadge } from '../WebhookActionMenu'
import { WebhookAuditTable } from '../WebhookAuditTable'
import { WebhookHealthCard } from '../WebhookHealthCard'
import { WebhookStatsCard } from '../WebhookStatsCard'
import * as webhooksBarrel from '../index'
import type { WebhookAuditLog } from '../../api'

describe('webhooks barrel', () => {
  it('re-exports all components', () => {
    expect(webhooksBarrel.WebhookStatsCard).toBeTruthy()
    expect(webhooksBarrel.WebhookAuditTable).toBeTruthy()
    expect(webhooksBarrel.BulkOperationModal).toBeTruthy()
    expect(webhooksBarrel.WebhookHealthCard).toBeTruthy()
    expect(webhooksBarrel.WebhookActionMenu).toBeTruthy()
    expect(webhooksBarrel.WebhookStatusBadge).toBeTruthy()
  })
})

describe('BulkOperationModal', () => {
  it('registers webhooks and shows results', async () => {
    const onConfirm = vi.fn().mockResolvedValue({
      total: 3,
      succeeded: 2,
      failed: 1,
      results: [
        { wallet_address: 'w1', success: true },
        { wallet_address: 'w2', success: true },
        { wallet_address: 'w3', success: false, error: 'duplicate' },
      ],
    })
    const onClose = vi.fn()
    render(
      <BulkOperationModal isOpen onClose={onClose} operation="register" onConfirm={onConfirm} />
    )
    expect(screen.getByText('Bulk Register Webhooks')).toBeInTheDocument()

    const textarea = screen.getByLabelText('Wallet Addresses') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: 'w1, w2\nw3' } })
    expect(screen.getByText('3 wallets detected')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('checkbox'))
    fireEvent.click(screen.getByRole('button', { name: /start registration/i }))

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith({
        wallets: ['w1', 'w2', 'w3'],
        force_recreate: true,
      })
    })
    expect(screen.getByText('Operation Complete')).toBeInTheDocument()
    expect(screen.getAllByText(/duplicate/).length).toBeGreaterThan(0)
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(onClose).toHaveBeenCalled()
  })

  it('cleans up webhooks with a full failure result', async () => {
    const onConfirm = vi.fn().mockResolvedValue({
      total: 1,
      succeeded: 0,
      failed: 1,
      results: [{ wallet_address: 'w1', success: false, error: 'nope' }],
    })
    const onClose = vi.fn()
    render(
      <BulkOperationModal isOpen onClose={onClose} operation="cleanup" onConfirm={onConfirm} />
    )
    expect(screen.getByText('Bulk Cleanup Webhooks')).toBeInTheDocument()
    const textarea = screen.getByLabelText('Wallet Addresses') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: 'w1' } })
    fireEvent.click(screen.getByRole('button', { name: /start cleanup/i }))
    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith({ wallets: ['w1'] })
    })
    expect(screen.getAllByText(/nope/).length).toBeGreaterThan(0)
  })

  it('shows the success icon when nothing failed', async () => {
    const onConfirm = vi.fn().mockResolvedValue({
      total: 1,
      succeeded: 1,
      failed: 0,
      results: [{ wallet_address: 'w1', success: true }],
    })
    render(
      <BulkOperationModal isOpen onClose={vi.fn()} operation="register" onConfirm={onConfirm} />
    )
    const textarea = screen.getByLabelText('Wallet Addresses') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: 'w1' } })
    fireEvent.click(screen.getByRole('button', { name: /start registration/i }))
    await waitFor(() => {
      expect(screen.getByText('Operation Complete')).toBeInTheDocument()
    })
  })

  it('does not confirm empty wallet lists', () => {
    const onConfirm = vi.fn()
    render(
      <BulkOperationModal isOpen onClose={vi.fn()} operation="register" onConfirm={onConfirm} />
    )
    const button = screen.getByRole('button', { name: /start registration/i })
    expect(button).toBeDisabled()
    fireEvent.click(button)
    expect(onConfirm).not.toHaveBeenCalled()
  })

  it('cancels without executing', () => {
    const onClose = vi.fn()
    render(
      <BulkOperationModal isOpen onClose={onClose} operation="register" onConfirm={vi.fn()} />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(onClose).toHaveBeenCalled()
  })

  it('logs errors when the operation fails', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const onConfirm = vi.fn().mockRejectedValue(new Error('boom'))
    render(
      <BulkOperationModal isOpen onClose={vi.fn()} operation="register" onConfirm={onConfirm} />
    )
    const textarea = screen.getByLabelText('Wallet Addresses') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: 'w1' } })
    fireEvent.click(screen.getByRole('button', { name: /start registration/i }))
    await waitFor(() => {
      expect(errorSpy).toHaveBeenCalled()
    })
    errorSpy.mockRestore()
  })
})

describe('WebhookActionMenu', () => {
  it('opens the menu and toggles a webhook', async () => {
    const onToggle = vi.fn().mockResolvedValue(undefined)
    const onRetry = vi.fn().mockResolvedValue(undefined)
    render(
      <WebhookActionMenu
        walletAddress="w1"
        isEnabled
        onToggle={onToggle}
        onRetry={onRetry}
      />
    )
    fireEvent.click(screen.getByRole('button'))
    expect(screen.getByText('Disable Webhook')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Disable Webhook'))
    await waitFor(() => {
      expect(onToggle).toHaveBeenCalledWith('w1', false)
    })
  })

  it('retries registration and closes on backdrop click', async () => {
    const onRetry = vi.fn().mockResolvedValue(undefined)
    render(
      <WebhookActionMenu walletAddress="w1" isEnabled={false} onToggle={vi.fn()} onRetry={onRetry} />
    )
    fireEvent.click(screen.getByRole('button'))
    expect(screen.getByText('Enable Webhook')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Retry Registration'))
    await waitFor(() => {
      expect(onRetry).toHaveBeenCalledWith('w1')
    })

    fireEvent.click(screen.getByRole('button'))
    const backdrop = document.querySelector('[role="presentation"]')
    fireEvent.keyDown(backdrop as HTMLElement, { key: 'Escape' })
    expect(backdrop).not.toBeInTheDocument()
  })

  it('disables actions while loading', () => {
    render(
      <WebhookActionMenu
        walletAddress="w1"
        onToggle={vi.fn()}
        onRetry={vi.fn()}
        isLoading
      />
    )
    expect(screen.getByRole('button')).toBeDisabled()
  })
})

describe('WebhookStatusBadge', () => {
  it('maps status and health combinations', () => {
    const { container } = render(<WebhookStatusBadge status="active" healthStatus="healthy" />)
    expect(container.textContent).toBe('active')
    render(<WebhookStatusBadge status="active" healthStatus="unhealthy" />)
    render(<WebhookStatusBadge status="active" healthStatus="error" />)
    render(<WebhookStatusBadge status="active" healthStatus="unknown" />)
    render(<WebhookStatusBadge status="paused" />)
    render(<WebhookStatusBadge status="failed" />)
    render(<WebhookStatusBadge status="orphaned" />)
  })
})

describe('WebhookAuditTable', () => {
  const logs: WebhookAuditLog[] = [
    {
      id: 1,
      wallet_address: 'wallet-address-1',
      action: 'register',
      status: 'success',
      webhook_id: 'webhook-id-1',
      details: 'registered ok',
      error_message: null,
      duration_ms: 50,
      created_at: '2025-01-01T00:00:00Z',
    },
    {
      id: 2,
      wallet_address: 'short',
      action: 'delete',
      status: 'failed',
      webhook_id: null,
      details: null,
      error_message: 'auth error',
      duration_ms: 1500,
      created_at: '2025-01-02T00:00:00Z',
    },
    {
      id: 3,
      wallet_address: 'a-very-long-wallet-address-1234567890',
      action: 'update',
      status: 'pending',
      webhook_id: 'another-long-webhook-id-9876543210',
      details: null,
      error_message: null,
      duration_ms: null,
      created_at: '2025-01-03T00:00:00Z',
    },
  ]

  it('renders the loading state', () => {
    render(<WebhookAuditTable logs={undefined} isLoading />)
    expect(screen.getByText('Loading audit log...')).toBeInTheDocument()
  })

  it('renders the empty state', () => {
    render(<WebhookAuditTable logs={[]} isLoading={false} />)
    expect(screen.getByText('No audit log entries found')).toBeInTheDocument()
    render(<WebhookAuditTable logs={undefined} isLoading={false} />)
  })

  it('renders rows and expands details', () => {
    render(<WebhookAuditTable logs={logs} isLoading={false} />)
    expect(screen.getByText('register')).toBeInTheDocument()
    expect(screen.getAllByText('-').length).toBeGreaterThan(0)
    expect(screen.getByText('1.50s')).toBeInTheDocument()
    expect(screen.getByText('50ms')).toBeInTheDocument()

    fireEvent.click(screen.getAllByRole('row')[1])
    expect(screen.getByText('registered ok')).toBeInTheDocument()
    fireEvent.click(screen.getAllByRole('row')[1])
    fireEvent.click(screen.getAllByRole('row')[2])
    expect(screen.getByText('auth error')).toBeInTheDocument()
  })
})

describe('WebhookHealthCard', () => {
  it('runs the health check', async () => {
    const onHealthCheck = vi.fn().mockResolvedValue({
      total_checked: 3,
      healthy: 2,
      unhealthy: 1,
      cleaned_up: 0,
      duration_ms: 10,
    })
    render(<WebhookHealthCard onHealthCheck={onHealthCheck} />)
    fireEvent.click(screen.getByRole('button', { name: /run health check/i }))
    await waitFor(() => {
      expect(onHealthCheck).toHaveBeenCalled()
    })
  })

  it('logs errors when the health check fails', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const onHealthCheck = vi.fn().mockRejectedValue(new Error('check failed'))
    render(<WebhookHealthCard onHealthCheck={onHealthCheck} />)
    fireEvent.click(screen.getByRole('button', { name: /run health check/i }))
    await waitFor(() => {
      expect(errorSpy).toHaveBeenCalledWith('Health check failed:', expect.anything())
    })
    errorSpy.mockRestore()
  })

  it('renders the loading state', () => {
    render(<WebhookHealthCard onHealthCheck={vi.fn()} isLoading />)
    expect(screen.getByText('Running Health Check...')).toBeInTheDocument()
  })
})

describe('WebhookStatsCard', () => {
  it('renders stats with data', () => {
    render(
      <WebhookStatsCard
        data={{ total_webhooks: 10, active_webhooks: 8, stale_webhooks: 1, failed_registrations: 1 }}
        isLoading={false}
      />
    )
    expect(screen.getByText('Total Webhooks')).toBeInTheDocument()
    expect(screen.getByText('10')).toBeInTheDocument()
  })

  it('renders without data and loading', () => {
    render(<WebhookStatsCard data={null} isLoading />)
    expect(screen.getAllByText('...').length).toBe(4)
    render(<WebhookStatsCard data={undefined} isLoading={false} />)
  })
})
