import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { AdminLogin } from '../AdminLogin'
import { LogoutButton } from '../LogoutButton'
import { useAuthStore } from '../../../stores/authStore'

const walletMock = vi.hoisted(() => ({
  useWallet: vi.fn(),
}))

const apiClientMock = vi.hoisted(() => ({
  post: vi.fn(),
}))

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('@solana/wallet-adapter-react', () => ({
  useWallet: walletMock.useWallet,
}))

vi.mock('../../../api/client', () => ({
  apiClient: apiClientMock,
}))

vi.mock('../../ui/Toast', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../ui/Toast')>()
  return {
    ...actual,
    toast: toastMock,
  }
})

function baseWallet(overrides: Record<string, unknown> = {}) {
  return {
    publicKey: { toBase58: () => 'wallet-address-123' },
    signMessage: vi.fn().mockResolvedValue(new Uint8Array(64).fill(1)),
    connected: true,
    disconnect: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
})

describe('AdminLogin', () => {
  it('shows a hint when no wallet is connected', () => {
    walletMock.useWallet.mockReturnValue({
      publicKey: null,
      signMessage: null,
      connected: false,
    })
    render(<AdminLogin />)
    expect(screen.getByText(/Connect a wallet to authenticate/i)).toBeInTheDocument()
  })

  it('shows an error when the wallet cannot sign messages', async () => {
    walletMock.useWallet.mockReturnValue(baseWallet({ signMessage: null }))
    render(<AdminLogin />)
    fireEvent.click(screen.getByRole('button', { name: /sign admin/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Please connect a Solana wallet first')
    })
  })

  it('authenticates an admin wallet successfully', async () => {
    const signMessage = vi.fn().mockResolvedValue(new Uint8Array(64).fill(2))
    walletMock.useWallet.mockReturnValue(baseWallet({ signMessage }))
    apiClientMock.post.mockResolvedValue({
      data: { token: 'jwt', role: 'admin', identifier: 'wallet-address-123' },
    })
    render(<AdminLogin />)
    fireEvent.click(screen.getByRole('button', { name: /sign admin/i }))
    await waitFor(() => {
      expect(apiClientMock.post).toHaveBeenCalledWith('/auth/wallet', expect.anything())
    })
    expect(useAuthStore.getState().user?.role).toBe('admin')
    expect(toastMock.success).toHaveBeenCalledWith('Admin wallet authenticated successfully')
  })

  it('signs with a 65-byte signature by trimming the flag byte', async () => {
    const signMessage = vi.fn().mockResolvedValue(new Uint8Array(65).fill(3))
    walletMock.useWallet.mockReturnValue(baseWallet({ signMessage }))
    apiClientMock.post.mockResolvedValue({
      data: { token: 'jwt', role: 'admin', identifier: 'w' },
    })
    render(<AdminLogin />)
    fireEvent.click(screen.getByRole('button', { name: /sign admin/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalled()
    })
  })

  it('rejects non-admin wallets', async () => {
    walletMock.useWallet.mockReturnValue(baseWallet())
    apiClientMock.post.mockResolvedValue({
      data: { token: 'jwt', role: 'operator', identifier: 'w' },
    })
    render(<AdminLogin />)
    fireEvent.click(screen.getByRole('button', { name: /sign admin/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Wallet does not have admin permissions')
    })
  })

  it('shows a 401/403 authorization error', async () => {
    walletMock.useWallet.mockReturnValue(baseWallet())
    apiClientMock.post.mockRejectedValue({ response: { status: 401 } })
    render(<AdminLogin />)
    fireEvent.click(screen.getByRole('button', { name: /sign admin/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith(
        'Wallet not authorized as admin on the backend'
      )
    })
  })

  it('shows a generic authentication failure', async () => {
    walletMock.useWallet.mockReturnValue(baseWallet())
    apiClientMock.post.mockRejectedValue({ message: 'network down' })
    render(<AdminLogin />)
    fireEvent.click(screen.getByRole('button', { name: /sign admin/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Authentication failed: network down')
    })
  })
})

describe('LogoutButton', () => {
  it('logs out, disconnects the wallet and navigates to login', async () => {
    const disconnect = vi.fn().mockResolvedValue(undefined)
    walletMock.useWallet.mockReturnValue({ disconnect, connected: true })
    useAuthStore.setState({
      user: { identifier: 'admin1', role: 'admin', token: 't' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 1000000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    render(
      <MemoryRouter>
        <LogoutButton />
      </MemoryRouter>
    )
    fireEvent.click(screen.getByRole('button', { name: /logout/i }))
    await waitFor(() => {
      expect(disconnect).toHaveBeenCalled()
    })
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
    expect(toastMock.success).toHaveBeenCalledWith('Logged out successfully')
  })

  it('logs the error when wallet disconnect fails', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const disconnect = vi.fn().mockRejectedValue(new Error('disconnect failed'))
    walletMock.useWallet.mockReturnValue({ disconnect, connected: true })
    useAuthStore.setState({
      user: { identifier: 'admin1', role: 'admin', token: 't' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 1000000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    render(
      <MemoryRouter>
        <LogoutButton />
      </MemoryRouter>
    )
    fireEvent.click(screen.getByRole('button', { name: /logout/i }))
    await waitFor(() => {
      expect(errorSpy).toHaveBeenCalledWith('Failed to disconnect wallet:', expect.anything())
    })
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
    errorSpy.mockRestore()
  })

  it('logs out without disconnecting when the wallet is not connected', () => {
    walletMock.useWallet.mockReturnValue({ disconnect: vi.fn(), connected: false })
    useAuthStore.setState({
      user: { identifier: 'admin1', role: 'admin', token: 't' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 1000000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    render(
      <MemoryRouter>
        <LogoutButton />
      </MemoryRouter>
    )
    fireEvent.click(screen.getByRole('button', { name: /logout/i }))
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
  })
})
