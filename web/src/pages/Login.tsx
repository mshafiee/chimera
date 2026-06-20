import { Navigate } from 'react-router-dom'
import { useAuthStore } from '../stores/authStore'
import { ConnectWalletButton } from '../components/wallet'
import { Button } from '../components/ui/Button'

// Dev mode flag - set to true to enable dev login button
const DEV_MODE = true

export function Login() {
  const { isAuthenticated, login } = useAuthStore()

  // Handle dev login
  const handleDevLogin = async () => {
    try {
      // For dev mode, use the pre-configured API key from the operator
      const DEV_API_KEY = 'dev-admin-key'

      // Test the API key by making a request to the health endpoint
      const response = await fetch('/api/v1/health', {
        headers: {
          'Authorization': `Bearer ${DEV_API_KEY}`,
          'Content-Type': 'application/json',
        },
      })

      if (response.ok) {
        // API key works - set auth state with the API key as the token
        const mockUser = {
          identifier: 'DevAdmin',
          role: 'admin' as const,
          token: DEV_API_KEY, // Use the API key as the token
        }

        // Call the login function from authStore
        login(mockUser, 86400) // 24 hours

        // Manually update localStorage to ensure persistence
        try {
          const currentAuth = localStorage.getItem('chimera-auth')
          const parsedAuth = currentAuth ? JSON.parse(currentAuth) : { state: {}, version: 0 }
          parsedAuth.state.user = mockUser
          parsedAuth.state.isAuthenticated = true
          parsedAuth.state.tokenExpiresAt = Date.now() + 86400000
          parsedAuth.state.lastActivity = Date.now()
          localStorage.setItem('chimera-auth', JSON.stringify(parsedAuth))
        } catch (e) {
          console.error('Failed to update localStorage:', e)
        }

        console.log('✅ Dev login successful')
      } else {
        console.error('❌ Dev API key failed:', response.status, response.statusText)
        alert('Dev login failed - make sure the operator is running')
      }
    } catch (error) {
      console.error('❌ Dev login error:', error)
      alert('Dev login failed - is the operator running?')
    }
  }

  // Redirect to dashboard if already authenticated
  if (isAuthenticated) {
    return <Navigate to="/dashboard" replace />
  }

  return (
    <div className="min-h-screen bg-background flex items-center justify-center">
      <div className="max-w-md w-full px-6">
        {/* Logo and Header */}
        <div className="text-center mb-8">
          <img src="/chimera.svg" alt="Chimera" className="w-16 h-16 mx-auto mb-4" />
          <h1 className="text-3xl font-bold bg-gradient-to-r from-shield to-spear bg-clip-text text-transparent">
            Chimera
          </h1>
          <p className="text-text-muted mt-2">High-Frequency Copy-Trading Platform</p>
        </div>

        {/* Login Card */}
        <div className="bg-surface border border-border rounded-xl p-8">
          <div className="text-center mb-6">
            <h2 className="text-xl font-semibold text-text mb-2">Connect Your Wallet</h2>
            <p className="text-text-muted text-sm">
              Connect your Solana wallet to access the Chimera dashboard
            </p>
          </div>

          {/* Wallet Connection */}
          <div className="flex justify-center mb-4">
            <ConnectWalletButton />
          </div>

          {/* Dev Login Button - Only shown in dev mode */}
          {DEV_MODE && (
            <div className="mt-4 pt-4 border-t border-border">
              <div className="text-center">
                <p className="text-xs text-text-muted mb-3">Development Mode</p>
                <Button
                  variant="shield"
                  size="sm"
                  onClick={handleDevLogin}
                  className="w-full"
                >
                  Dev Login (No Wallet)
                </Button>
              </div>
            </div>
          )}

          {/* Info */}
          <div className="mt-6 p-4 bg-surface-light rounded-lg">
            <p className="text-xs text-text-muted text-center">
              Your wallet will be used to sign a message for authentication.
              No transactions will be executed without your explicit authorization.
            </p>
          </div>
        </div>

        {/* Footer */}
        <div className="text-center mt-8 text-text-muted text-sm">
          <p>Chimera v7.1 - MoSch Engineering</p>
        </div>
      </div>
    </div>
  )
}
