import { useState } from 'react'
import axios from 'axios'
import { useAuthStore } from '../../stores/authStore'
import { toast } from '../ui/Toast'
import { Button } from '../ui/Button'
import { Lock } from 'lucide-react'

export function AdminLogin() {
  const [walletAddress, setWalletAddress] = useState('')
  const [loading, setLoading] = useState(false)
  const { login } = useAuthStore()

  const handleLogin = async () => {
    if (!walletAddress.trim()) {
      toast.error('Please enter a wallet address')
      return
    }

    // Validate Solana address format (base58, 32-44 chars)
    if (walletAddress.length < 32 || walletAddress.length > 44) {
      toast.error('Invalid wallet address format')
      return
    }

    setLoading(true)
    try {
      // Test authentication by making a simple API call with wallet address as Bearer token
      // The backend will check if this wallet is in admin_wallets (from config)
      const testClient = axios.create({
        baseURL: '/api/v1',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${walletAddress.trim()}`,
        },
      })

      // Try to get config to verify admin access
      await testClient.get('/config')
      
      // If successful, the wallet is authenticated as admin
      // Store the wallet address as the token (backend uses it directly)
      // IMPORTANT: Use wallet address as token, not JWT
      login({
        identifier: walletAddress.trim(),
        role: 'admin', // Config endpoint requires admin, so role is admin
        token: walletAddress.trim(), // Use wallet address as Bearer token (not JWT)
      })

      toast.success('Admin wallet authenticated successfully')
      setWalletAddress('')
      
      // Force a page refresh to ensure all components use the new auth state
      // This prevents stale JWT tokens from being used
      window.location.reload()
    } catch (error: any) {
      if (error.response?.status === 401 || error.response?.status === 403) {
        toast.error('Wallet not authorized. Please check if it is configured as admin in config.yaml')
      } else {
        toast.error('Authentication failed. Please try again.')
      }
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="flex items-center gap-2">
      <input
        type="text"
        value={walletAddress}
        onChange={(e) => setWalletAddress(e.target.value)}
        placeholder="Enter admin wallet address"
        className="bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text focus:outline-none focus:ring-2 focus:ring-shield min-w-[300px]"
        onKeyPress={(e) => {
          if (e.key === 'Enter') {
            handleLogin()
          }
        }}
      />
      <Button
        variant="primary"
        size="sm"
        onClick={handleLogin}
        loading={loading}
        disabled={!walletAddress.trim() || loading}
      >
        <Lock className="w-4 h-4 mr-2" />
        Admin Login
      </Button>
    </div>
  )
}
