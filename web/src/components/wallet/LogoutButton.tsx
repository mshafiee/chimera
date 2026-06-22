import { LogOut } from 'lucide-react'
import { useWallet } from '@solana/wallet-adapter-react'
import { useNavigate } from 'react-router-dom'
import { useAuthStore } from '../../stores/authStore'
import { Button } from '../ui/Button'
import { toast } from '../ui/Toast'

export function LogoutButton() {
  const { logout, user } = useAuthStore()
  const { disconnect, connected } = useWallet()
  const navigate = useNavigate()

  const handleLogout = () => {
    const isWalletConnected = connected

    // If wallet is connected and user was authenticated via wallet, disconnect wallet
    if (isWalletConnected) {
      disconnect().catch((error) => {
        console.error('Failed to disconnect wallet:', error)
      })
    }

    // Clear authentication state
    logout()
    toast.success('Logged out successfully')
    navigate('/login', { replace: true })
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
