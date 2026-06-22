import { useMemo, ReactNode, useCallback, useEffect } from 'react'
import {
  ConnectionProvider,
  WalletProvider as SolanaWalletProvider,
  useWallet,
} from '@solana/wallet-adapter-react'
import { WalletModalProvider, WalletMultiButton } from '@solana/wallet-adapter-react-ui'
import { PhantomWalletAdapter } from '@solana/wallet-adapter-phantom'
import { SolflareWalletAdapter } from '@solana/wallet-adapter-solflare'
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
      const message = `Chimera Dashboard Authentication\n\nWallet: ${publicKey.toBase58()}\nTimestamp: ${Math.floor(Date.now() / 1000)}`
      const encodedMessage = new TextEncoder().encode(message)
      
      // Sign the message
      const signature = await signMessage(encodedMessage)

      // Send to backend for verification
      // Convert signature to URL-safe base64 without padding (as expected by backend)
      // Handle both 64-byte and 65-byte (with legacy flag) Solana signatures
      // Use browser-compatible base64 encoding
      const signatureBytes = signature.length === 65 ? signature.slice(0, 64) : signature
      // Convert Uint8Array to binary string, then to base64
      let binary = ''
      const len = signatureBytes.byteLength
      for (let i = 0; i < len; i++) {
        binary += String.fromCharCode(signatureBytes[i])
      }
      const signatureBase64 = btoa(binary)
        .replace(/\+/g, '-')
        .replace(/\//g, '_')
        .replace(/=/g, '')

      if (import.meta.env.DEV) {
        console.log('[Auth] Wallet:', publicKey.toBase58(), 'Signature:', signatureBase64.substring(0, 20) + '...')
      }

      const response = await apiClient.post<{
        token: string
        role: string
        identifier: string
      }>('/auth/wallet', {
        wallet_address: publicKey.toBase58(),
        message,
        signature: signatureBase64,
      })

      // Store auth state
      login({
        identifier: response.data.identifier,
        role: response.data.role as 'readonly' | 'operator' | 'admin',
        token: response.data.token,
      })
      toast.success('Wallet authenticated successfully')
    } catch (error: any) {
      if (import.meta.env.DEV) {
        console.error('[Auth] Failed:', error)
      }
      toast.error(`Authentication failed: ${error.response?.data?.reason || error.message || 'Unknown error'}`)
      // Disconnect wallet on auth failure
      disconnect()
    }
  }, [publicKey, signMessage, login, disconnect])

  // Handle wallet connection/disconnection and wallet change
  useEffect(() => {
    if (!connected || !publicKey) {
      if (isAuthenticated && user) {
        logout()
      }
      return
    }

    if (!isAuthenticated) {
      authenticate()
    } else if (user && user.identifier !== publicKey.toBase58()) {
      logout()
    }
  }, [connected, publicKey, isAuthenticated, user])

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
