import { Outlet } from 'react-router-dom'
import { Sidebar } from './Sidebar'
import { Header } from './Header'
import { useState, useCallback } from 'react'

export function Layout() {
  const [lastUpdate, setLastUpdate] = useState<Date | null>(null)
  const [isConnected] = useState(false) // Will be set by WebSocket hook

  const handleRefresh = useCallback(() => {
    setLastUpdate(new Date())
    // Trigger refetch of current page data
    window.dispatchEvent(new CustomEvent('chimera:refresh'))
  }, [])

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <div className="ml-64">
        <Header 
          isConnected={isConnected} 
          lastUpdate={lastUpdate}
          onRefresh={handleRefresh}
        />
        <main className="p-6">
          <Outlet context={{ setLastUpdate }} />
        </main>
      </div>
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
