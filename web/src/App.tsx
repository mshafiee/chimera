import { lazy, Suspense } from 'react'
import { Routes, Route, Navigate } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { ProtectedRoute } from './components/auth/ProtectedRoute'
import { LoadingSpinner } from './components/ui/LoadingSpinner'

const Login = lazy(() => import('./pages/Login').then(m => ({ default: m.Login })))
const Dashboard = lazy(() => import('./pages/Dashboard').then(m => ({ default: m.Dashboard })))
const Wallets = lazy(() => import('./pages/Wallets').then(m => ({ default: m.Wallets })))
const Trades = lazy(() => import('./pages/Trades').then(m => ({ default: m.Trades })))
const Config = lazy(() => import('./pages/Config').then(m => ({ default: m.Config })))
const Incidents = lazy(() => import('./pages/Incidents').then(m => ({ default: m.Incidents })))
const Scout = lazy(() => import('./pages/Scout').then(m => ({ default: m.Scout })))
const Signals = lazy(() => import('./pages/Signals').then(m => ({ default: m.Signals })))
const Market = lazy(() => import('./pages/Market').then(m => ({ default: m.Market })))
const Risk = lazy(() => import('./pages/Risk').then(m => ({ default: m.Risk })))
const Reconciliation = lazy(() => import('./pages/Reconciliation').then(m => ({ default: m.Reconciliation })))
const Performance = lazy(() => import('./pages/Performance').then(m => ({ default: m.Performance })))
const Operations = lazy(() => import('./pages/Operations').then(m => ({ default: m.Operations })))
const Consensus = lazy(() => import('./pages/Consensus').then(m => ({ default: m.Consensus })))
const WalletMonitoring = lazy(() => import('./pages/WalletMonitoring').then(m => ({ default: m.WalletMonitoring })))
const Webhooks = lazy(() => import('./pages/Webhooks').then(m => ({ default: m.Webhooks })))

const SuspenseWrapper = ({ children }: { children: React.ReactNode }) => (
  <Suspense fallback={<LoadingSpinner />}>{children}</Suspense>
)

function App() {
  return (
    <SuspenseWrapper>
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
        <Route path="webhooks" element={<Webhooks />} />
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
    </SuspenseWrapper>
  )
}

export default App
