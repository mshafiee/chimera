import { useMemo, ReactNode, useCallback, useEffect } from 'react'
import {
  ConnectionProvider,
  WalletProvider as SolanaWalletProvider,
  useWallet,
} from '@solana/wallet-adapter-react'
import { WalletModalProvider, WalletMultiButton } from '@solana/wallet-adapter-react-ui'
import { PhantomWalletAdapter, SolflareWalletAdapter } from '@solana/wallet-adapter-wallets'
import { clusterApiUrl } from '@solana/web3.js'
import { useAuthStore } from '../../stores/authStore'
import { apiClient } from '../../api/client'
import { toast } from '../ui/Toast'

// Import wallet adapter CSS
import '@solana/wallet-adapter-react-ui/styles.css'

interface WalletProviderProps {
  children: ReactNode
}

// Solana RPC endpoint
const SOLANA_NETWORK = 'mainnet-beta'
const SOLANA_RPC = import.meta.env.VITE_SOLANA_RPC || clusterApiUrl(SOLANA_NETWORK)

export function WalletProvider({ children }: WalletProviderProps) {
  const wallets = useMemo(
    () => [
      new PhantomWalletAdapter(),
      new SolflareWalletAdapter(),
    ],
    []
  )

  return (
    <ConnectionProvider endpoint={SOLANA_RPC}>
      <SolanaWalletProvider wallets={wallets} autoConnect>
        <WalletModalProvider>
          <WalletAuthProvider>{children}</WalletAuthProvider>
        </WalletModalProvider>
      </SolanaWalletProvider>
    </ConnectionProvider>
  )
}

// Inner component that handles auth
function WalletAuthProvider({ children }: { children: ReactNode }) {
  const { publicKey, signMessage, connected, disconnect } = useWallet()
  const { login, logout, isAuthenticated, user } = useAuthStore()

  // Handle wallet connection and authentication
  const authenticate = useCallback(async () => {
    if (!publicKey || !signMessage) return

    try {
      // Create a message to sign
      const message = `Chimera Dashboard Authentication\n\nWallet: ${publicKey.toBase58()}\nTimestamp: ${Date.now()}`
      const encodedMessage = new TextEncoder().encode(message)
      
      // Sign the message
      const signature = await signMessage(encodedMessage)
      
      // Send to backend for verification
      const response = await apiClient.post<{
        token: string
        role: string
        identifier: string
      }>('/auth/wallet', {
        wallet_address: publicKey.toBase58(),
        message,
        signature: Buffer.from(signature).toString('base64'),
      })

      // Store auth state
      login({
        identifier: response.data.identifier,
        role: response.data.role as 'readonly' | 'operator' | 'admin',
        token: response.data.token,
      })
      toast.success('Wallet authenticated successfully')
    } catch (error) {
      console.error('Authentication failed:', error)
      toast.error('Authentication failed. Please try again.')
      // Disconnect wallet on auth failure
      disconnect()
    }
  }, [publicKey, signMessage, login, disconnect])

  // Handle wallet connection/disconnection
  useEffect(() => {
    if (connected && publicKey && !isAuthenticated) {
      // Wallet connected but not authenticated - trigger auth
      authenticate()
    } else if (!connected && isAuthenticated && user) {
      // Wallet disconnected - only logout if user was authenticated via wallet signature (JWT token)
      // Admin login uses wallet address directly as token (no JWT), so preserve it
      // Check if token is a JWT (contains dots) vs wallet address (no dots, base58)
      const isJwtToken = user.token.includes('.')
      if (isJwtToken && user.identifier === publicKey?.toBase58()) {
        // JWT token and identifier matches wallet - this was wallet-based auth, logout
        logout()
      }
      // If token is not JWT (wallet address), it's admin login - preserve it
    }
  }, [connected, publicKey, isAuthenticated, user, authenticate, logout])

  // Handle wallet change (only for wallet-based auth, not admin login)
  useEffect(() => {
    if (connected && publicKey && isAuthenticated && user) {
      // Only handle wallet change for JWT-based auth (wallet signature)
      // Admin login uses wallet address as token, so don't interfere
      const isJwtToken = user.token.includes('.')
      if (isJwtToken && user.identifier !== publicKey.toBase58()) {
        // Different wallet connected - re-authenticate
        logout()
        authenticate()
      }
    }
  }, [connected, publicKey, isAuthenticated, user, authenticate, logout])

  return <>{children}</>
}

// Custom styled wallet button
export function ConnectWalletButton() {
  return (
    <div className="wallet-adapter-button-wrapper">
      <WalletMultiButton className="!bg-shield hover:!bg-shield-dark !text-background !font-medium !rounded-lg !h-10" />
    </div>
  )
}
