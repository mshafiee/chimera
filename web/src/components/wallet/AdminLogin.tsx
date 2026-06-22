import { useCallback } from 'react'
import { useWallet } from '@solana/wallet-adapter-react'
import { useAuthStore } from '../../stores/authStore'
import { apiClient } from '../../api/client'
import { toast } from '../ui/Toast'
import { Button } from '../ui/Button'
import { Lock, Wallet } from 'lucide-react'

export function AdminLogin() {
  const { publicKey, signMessage, connected } = useWallet()
  const { login } = useAuthStore()

  const handleLogin = useCallback(async () => {
    if (!publicKey || !signMessage) {
      toast.error('Please connect a Solana wallet first')
      return
    }

    try {
      const walletAddress = publicKey.toBase58()
      const message = `Chimera Dashboard Authentication\n\nWallet: ${walletAddress}\nTimestamp: ${Math.floor(Date.now() / 1000)}`
      const encodedMessage = new TextEncoder().encode(message)
      const signature = await signMessage(encodedMessage)

      const signatureBytes = signature.length === 65 ? signature.slice(0, 64) : signature
      let binary = ''
      for (let i = 0; i < signatureBytes.byteLength; i++) {
        binary += String.fromCharCode(signatureBytes[i])
      }
      const signatureBase64 = btoa(binary)
        .replace(/\+/g, '-')
        .replace(/\//g, '_')
        .replace(/=/g, '')

      const response = await apiClient.post<{
        token: string
        role: string
        identifier: string
      }>('/auth/wallet', {
        wallet_address: walletAddress,
        message,
        signature: signatureBase64,
      })

      if (response.data.role !== 'admin') {
        toast.error('Wallet does not have admin permissions')
        return
      }

      login({
        identifier: response.data.identifier,
        role: 'admin',
        token: response.data.token,
      })

      toast.success('Admin wallet authenticated successfully')
    } catch (error: any) {
      if (error.response?.status === 401 || error.response?.status === 403) {
        toast.error('Wallet not authorized as admin on the backend')
      } else {
        toast.error(`Authentication failed: ${error.message ?? 'Unknown error'}`)
      }
    }
  }, [publicKey, signMessage, login])

  if (!connected || !publicKey) {
    return (
      <div className="flex items-center gap-2 text-sm text-text-muted">
        <Wallet className="w-4 h-4" />
        <span>Connect a wallet to authenticate as admin</span>
      </div>
    )
  }

  return (
    <div className="flex items-center gap-2">
      <span className="text-xs text-text-muted truncate max-w-[160px]">
        {publicKey.toBase58().slice(0, 8)}...
      </span>
      <Button variant="primary" size="sm" onClick={handleLogin}>
        <Lock className="w-4 h-4 mr-2" />
        Sign Admin
      </Button>
    </div>
  )
}
