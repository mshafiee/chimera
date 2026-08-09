import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { WalletProvider, ConnectWalletButton } from '../WalletProvider'
import * as walletBarrel from '../index'
import { useAuthStore } from '../../../stores/authStore'

const useWalletMock = vi.hoisted(() => vi.fn())
const apiClientMock = vi.hoisted(() => ({ post: vi.fn() }))
const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('@solana/wallet-adapter-react', () => ({
  ConnectionProvider: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  WalletProvider: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  useWallet: useWalletMock,
}))

vi.mock('@solana/wallet-adapter-react-ui', () => ({
  WalletModalProvider: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  WalletMultiButton: () => <button>Connect Wallet</button>,
}))

vi.mock('@solana/wallet-adapter-phantom', () => ({
  PhantomWalletAdapter: class {},
}))

vi.mock('@solana/wallet-adapter-solflare', () => ({
  SolflareWalletAdapter: class {},
}))

vi.mock('@solana/web3.js', () => ({
  clusterApiUrl: () => 'https://api.mainnet-beta.solana.com',
}))

vi.mock('../../../api/client', () => ({
  apiClient: apiClientMock,
}))

vi.mock('../../ui/Toast', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../ui/Toast')>()
  return { ...actual, toast: toastMock }
})

const walletState = {
  publicKey: null as null | { toBase58: () => string },
  signMessage: null as null | ((m: Uint8Array) => Promise<Uint8Array>),
  connected: false,
  disconnect: vi.fn().mockResolvedValue(undefined),
}

function resetAuth() {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  resetAuth()
  walletState.publicKey = null
  walletState.signMessage = null
  walletState.connected = false
  walletState.disconnect = vi.fn().mockResolvedValue(undefined)
  useWalletMock.mockReturnValue(walletState)
})

describe('wallet barrel', () => {
  it('re-exports all components', () => {
    expect(walletBarrel.WalletProvider).toBeTruthy()
    expect(walletBarrel.ConnectWalletButton).toBeTruthy()
    expect(walletBarrel.LogoutButton).toBeTruthy()
  })
})

describe('WalletProvider', () => {
  it('renders children and the connect button', () => {
    render(
      <WalletProvider>
        <div>Child content</div>
      </WalletProvider>
    )
    expect(screen.getByText('Child content')).toBeInTheDocument()
  })
})

describe('WalletAuthProvider (via WalletProvider)', () => {
  it('logs out when an authenticated user disconnects the wallet', () => {
    useAuthStore.setState({
      user: { identifier: 'wallet-1', role: 'admin', token: 'tok' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 3600000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    render(
      <WalletProvider>
        <div>Content</div>
      </WalletProvider>
    )
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
  })

  it('authenticates a connected wallet with a signature', async () => {
    walletState.publicKey = { toBase58: () => 'wallet-1' }
    walletState.signMessage = vi.fn().mockResolvedValue(new Uint8Array(64).fill(7))
    walletState.connected = true
    apiClientMock.post.mockResolvedValue({
      data: { token: 'jwt', role: 'admin', identifier: 'wallet-1' },
    })
    render(
      <WalletProvider>
        <div>Content</div>
      </WalletProvider>
    )
    await waitFor(() => {
      expect(apiClientMock.post).toHaveBeenCalledWith('/auth/wallet', expect.anything())
    })
    expect(useAuthStore.getState().user?.token).toBe('jwt')
    expect(toastMock.success).toHaveBeenCalledWith('Wallet authenticated successfully')
  })

  it('does not authenticate without a signer', () => {
    walletState.publicKey = { toBase58: () => 'wallet-1' }
    walletState.connected = true
    render(
      <WalletProvider>
        <div>Content</div>
      </WalletProvider>
    )
    expect(apiClientMock.post).not.toHaveBeenCalled()
  })

  it('disconnects and toasts when authentication fails', async () => {
    walletState.publicKey = { toBase58: () => 'wallet-1' }
    walletState.signMessage = vi.fn().mockResolvedValue(new Uint8Array(64).fill(1))
    walletState.connected = true
    apiClientMock.post.mockRejectedValue({
      response: { data: { reason: 'not authorized' } },
    })
    render(
      <WalletProvider>
        <div>Content</div>
      </WalletProvider>
    )
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Authentication failed: not authorized')
    })
    expect(walletState.disconnect).toHaveBeenCalled()
  })

  it('logs out when the wallet changes to a different user', () => {
    useAuthStore.setState({
      user: { identifier: 'wallet-1', role: 'admin', token: 'tok' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 3600000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    walletState.publicKey = { toBase58: () => 'wallet-2' }
    walletState.connected = true
    render(
      <WalletProvider>
        <div>Content</div>
      </WalletProvider>
    )
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
  })
})

describe('ConnectWalletButton', () => {
  it('renders the wallet multi button', () => {
    render(<ConnectWalletButton />)
    expect(screen.getByRole('button', { name: 'Connect Wallet' })).toBeInTheDocument()
  })
})
