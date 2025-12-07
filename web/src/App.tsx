import { Routes, Route, Navigate } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { Dashboard } from './pages/Dashboard'
import { Wallets } from './pages/Wallets'
import { Trades } from './pages/Trades'
import { Config } from './pages/Config'
import { Incidents } from './pages/Incidents'

function App() {
  return (
    <Routes>
      <Route path="/" element={<Layout />}>
        <Route index element={<Navigate to="/dashboard" replace />} />
        <Route path="dashboard" element={<Dashboard />} />
        <Route path="wallets" element={<Wallets />} />
        <Route path="trades" element={<Trades />} />
        <Route path="config" element={<Config />} />
        <Route path="incidents" element={<Incidents />} />
      </Route>
    </Routes>
  )
}

export default App
