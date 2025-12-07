import { NavLink } from 'react-router-dom'
import { clsx } from 'clsx'
import {
  LayoutDashboard,
  Wallet,
  ArrowLeftRight,
  Settings,
  AlertTriangle,
} from 'lucide-react'

const navigation = [
  { name: 'Dashboard', href: '/dashboard', icon: LayoutDashboard },
  { name: 'Wallets', href: '/wallets', icon: Wallet },
  { name: 'Trades', href: '/trades', icon: ArrowLeftRight },
  { name: 'Config', href: '/config', icon: Settings },
  { name: 'Incidents', href: '/incidents', icon: AlertTriangle },
]

export function Sidebar() {
  return (
    <aside className="fixed inset-y-0 left-0 w-64 bg-surface border-r border-border flex flex-col">
      {/* Logo */}
      <div className="h-16 flex items-center px-6 border-b border-border">
        <div className="flex items-center gap-3">
          <img src="/chimera.svg" alt="Chimera" className="w-8 h-8" />
          <span className="text-xl font-bold bg-gradient-to-r from-shield to-spear bg-clip-text text-transparent">
            CHIMERA
          </span>
        </div>
      </div>

      {/* Navigation */}
      <nav className="flex-1 py-4 px-3 space-y-1">
        {navigation.map((item) => (
          <NavLink
            key={item.name}
            to={item.href}
            className={({ isActive }) =>
              clsx(
                'flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm font-medium transition-all',
                isActive
                  ? 'bg-shield/10 text-shield border border-shield/30'
                  : 'text-text-muted hover:text-text hover:bg-surface-light'
              )
            }
          >
            <item.icon className="w-5 h-5" />
            {item.name}
          </NavLink>
        ))}
      </nav>

      {/* Footer */}
      <div className="p-4 border-t border-border">
        <div className="text-xs text-text-muted">
          <div>Chimera v7.1</div>
          <div className="mt-1">© 2025 Project Chimera</div>
        </div>
      </div>
    </aside>
  )
}
