import { Routes, Route, Navigate } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { ProtectedRoute } from './components/auth/ProtectedRoute'
import { Login } from './pages/Login'
import { Dashboard } from './pages/Dashboard'
import { Wallets } from './pages/Wallets'
import { Trades } from './pages/Trades'
import { Config } from './pages/Config'
import { Incidents } from './pages/Incidents'

// New pages
import { Scout } from './pages/Scout'
import { Signals } from './pages/Signals'
import { Market } from './pages/Market'
import { Risk } from './pages/Risk'
import { Reconciliation } from './pages/Reconciliation'
import { Performance } from './pages/Performance'
import { Operations } from './pages/Operations'
import { Consensus } from './pages/Consensus'
import { WalletMonitoring } from './pages/WalletMonitoring'

function App() {
  return (
    <Routes>
      {/* Public route - Login page */}
      <Route path="/login" element={<Login />} />

      {/* Protected routes - require authentication */}
      <Route path="/" element={
        <ProtectedRoute>
          <Layout />
        </ProtectedRoute>
      }>
        <Route index element={<Navigate to="/dashboard" replace />} />
        <Route path="dashboard" element={<Dashboard />} />
        <Route path="wallets" element={<Wallets />} />
        <Route path="wallet-monitoring" element={<WalletMonitoring />} />
        <Route path="trades" element={<Trades />} />
        <Route path="incidents" element={<Incidents />} />
        {/* New pages */}
        <Route path="scout" element={<Scout />} />
        <Route path="signals" element={<Signals />} />
        <Route path="market" element={<Market />} />
        <Route path="risk" element={<Risk />} />
        <Route path="reconciliation" element={<Reconciliation />} />
        <Route path="performance" element={<Performance />} />
        <Route path="operations" element={<Operations />} />
        <Route path="consensus" element={<Consensus />} />

        {/* Admin-only routes */}
        <Route path="config" element={
          <ProtectedRoute requireRole="admin">
            <Config />
          </ProtectedRoute>
        } />
      </Route>

      {/* Catch all - redirect to dashboard or login */}
      <Route path="*" element={<Navigate to="/dashboard" replace />} />
    </Routes>
  )
}

export default App
