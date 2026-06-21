import { Navigate } from 'react-router-dom'
import { useAuthStore } from '../stores/authStore'
import { ConnectWalletButton } from '../components/wallet'

export function Login() {
  const { isAuthenticated } = useAuthStore()

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
