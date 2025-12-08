import { Outlet } from 'react-router-dom'
import { Sidebar, MobileBottomNav } from './Sidebar'
import { Header } from './Header'
import { useState, useCallback } from 'react'
import { Menu, X } from 'lucide-react'
import { ToastContainer, useToastStore } from '../ui/Toast'
import { useWebSocket } from '../../hooks/useWebSocket'

export function Layout() {
  const [lastUpdate, setLastUpdate] = useState<Date | null>(null)
  const { isConnected } = useWebSocket() // Get actual WebSocket connection status
  const [isMobileMenuOpen, setIsMobileMenuOpen] = useState(false)

  const handleRefresh = useCallback(() => {
    setLastUpdate(new Date())
    // Trigger refetch of current page data
    window.dispatchEvent(new CustomEvent('chimera:refresh'))
  }, [])

  const toggleMobileMenu = useCallback(() => {
    setIsMobileMenuOpen((prev) => !prev)
  }, [])

  const closeMobileMenu = useCallback(() => {
    setIsMobileMenuOpen(false)
  }, [])

  return (
    <div className="min-h-screen bg-background">
      {/* Desktop Sidebar - hidden on mobile */}
      <div className="hidden md:block">
        <Sidebar />
      </div>

      {/* Mobile Menu Overlay */}
      {isMobileMenuOpen && (
        <div
          className="fixed inset-0 z-40 bg-black/50 md:hidden"
          onClick={closeMobileMenu}
        />
      )}

      {/* Mobile Sidebar - slides in from left */}
      <div
        className={`fixed inset-y-0 left-0 z-50 w-64 transform transition-transform duration-300 ease-in-out md:hidden ${
          isMobileMenuOpen ? 'translate-x-0' : '-translate-x-full'
        }`}
      >
        <Sidebar onNavigate={closeMobileMenu} />
      </div>

      {/* Main Content */}
      <div className="md:ml-64">
        {/* Mobile Header with menu toggle */}
        <div className="md:hidden flex items-center h-16 px-4 border-b border-border bg-surface">
          <button
            onClick={toggleMobileMenu}
            className="p-2 -ml-2 text-text-muted hover:text-text"
            aria-label="Toggle menu"
          >
            {isMobileMenuOpen ? (
              <X className="w-6 h-6" />
            ) : (
              <Menu className="w-6 h-6" />
            )}
          </button>
          <div className="ml-3 flex items-center gap-2">
            <img src="/chimera.svg" alt="Chimera" className="w-6 h-6" />
            <span className="text-lg font-bold bg-gradient-to-r from-shield to-spear bg-clip-text text-transparent">
              CHIMERA
            </span>
          </div>
        </div>

        {/* Desktop Header */}
        <div className="hidden md:block">
          <Header
            isConnected={isConnected}
            lastUpdate={lastUpdate}
            onRefresh={handleRefresh}
          />
        </div>

        {/* Page Content */}
        <main className="p-4 md:p-6 pb-20 md:pb-6">
          <Outlet context={{ setLastUpdate }} />
        </main>
      </div>

      {/* Mobile Bottom Navigation */}
      <MobileBottomNav />

      {/* Toast Notifications */}
      <ToastContainer
        toasts={useToastStore((state) => state.toasts)}
        onClose={(id) => useToastStore.getState().removeToast(id)}
      />
    </div>
  )
}

// Hook to get layout context in child pages
import { useOutletContext } from 'react-router-dom'

interface LayoutContext {
  setLastUpdate: (date: Date) => void
}

export function useLayoutContext() {
  return useOutletContext<LayoutContext>()
}
