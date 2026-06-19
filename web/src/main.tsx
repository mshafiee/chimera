import React from 'react'
import ReactDOM from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { WalletProvider } from './components/wallet'
import { useActivityTracker } from './hooks/useActivityTracker'
import App from './App'
import './index.css'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5000,
      refetchOnWindowFocus: false,
    },
  },
})

// Activity tracker wrapper component
function AppWithActivityTracker() {
  // Enable activity tracking for authenticated users
  useActivityTracker(true)
  return <App />
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <WalletProvider>
        <BrowserRouter
          future={{
            v7_startTransition: true,
            v7_relativeSplatPath: true,
          }}
        >
          <AppWithActivityTracker />
        </BrowserRouter>
      </WalletProvider>
    </QueryClientProvider>
  </React.StrictMode>,
)
