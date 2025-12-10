import { LogOut } from 'lucide-react'
import { useWallet } from '@solana/wallet-adapter-react'
import { useAuthStore } from '../../stores/authStore'
import { Button } from '../ui/Button'
import { toast } from '../ui/Toast'

export function LogoutButton() {
  const { logout, user } = useAuthStore()
  const { disconnect, connected } = useWallet()

  const handleLogout = () => {
    // Check if this is wallet-based auth (JWT token) or admin login (wallet address)
    const isJwtToken = user?.token?.includes('.') ?? false

    // If wallet is connected and this was wallet-based auth, disconnect wallet
    if (connected && isJwtToken) {
      disconnect().catch((error) => {
        console.error('Failed to disconnect wallet:', error)
      })
    }

    // Clear authentication state
    logout()
    toast.success('Logged out successfully')
    
    // Force a page refresh to ensure all components use the cleared auth state
    // This prevents stale tokens from being used
    setTimeout(() => {
      window.location.reload()
    }, 500)
  }

  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={handleLogout}
      className="flex items-center gap-2"
      title={`Logout ${user?.identifier ? `(${user.identifier.slice(0, 8)}...)` : ''}`}
    >
      <LogOut className="w-4 h-4" />
      <span className="hidden sm:inline">Logout</span>
    </Button>
  )
}
