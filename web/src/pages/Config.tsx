import { useState, useEffect } from 'react'
import { AlertTriangle, Save, RefreshCw } from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { ConfirmModal } from '../components/ui/Modal'
import { useConfig, useUpdateConfig, useResetCircuitBreaker, useHealth } from '../api'
import { useAuthStore } from '../stores/authStore'

export function Config() {
  const { hasPermission } = useAuthStore()
  const isAdmin = hasPermission('admin')

  const { data: config, isLoading } = useConfig()
  const { data: health } = useHealth()
  const updateConfig = useUpdateConfig()
  const resetCircuitBreaker = useResetCircuitBreaker()

  // Local form state
  const [maxLoss24h, setMaxLoss24h] = useState(0)
  const [maxConsecutiveLosses, setMaxConsecutiveLosses] = useState(0)
  const [maxDrawdown, setMaxDrawdown] = useState(0)
  const [cooldownMinutes, setCooldownMinutes] = useState(0)
  const [shieldPercent, setShieldPercent] = useState(70)

  const [showResetConfirm, setShowResetConfirm] = useState(false)
  const [hasChanges, setHasChanges] = useState(false)

  // Initialize form from config
  useEffect(() => {
    if (config) {
      setMaxLoss24h(config.circuit_breakers.max_loss_24h)
      setMaxConsecutiveLosses(config.circuit_breakers.max_consecutive_losses)
      setMaxDrawdown(config.circuit_breakers.max_drawdown_percent)
      setCooldownMinutes(config.circuit_breakers.cool_down_minutes)
      setShieldPercent(config.strategy_allocation.shield_percent)
    }
  }, [config])

  // Track changes
  useEffect(() => {
    if (config) {
      const changed =
        maxLoss24h !== config.circuit_breakers.max_loss_24h ||
        maxConsecutiveLosses !== config.circuit_breakers.max_consecutive_losses ||
        maxDrawdown !== config.circuit_breakers.max_drawdown_percent ||
        cooldownMinutes !== config.circuit_breakers.cool_down_minutes ||
        shieldPercent !== config.strategy_allocation.shield_percent
      setHasChanges(changed)
    }
  }, [config, maxLoss24h, maxConsecutiveLosses, maxDrawdown, cooldownMinutes, shieldPercent])

  const handleSaveCircuitBreakers = async () => {
    await updateConfig.mutateAsync({
      circuit_breakers: {
        max_loss_24h: maxLoss24h,
        max_consecutive_losses: maxConsecutiveLosses,
        max_drawdown_percent: maxDrawdown,
        cool_down_minutes: cooldownMinutes,
      },
    })
  }

  const handleSaveAllocation = async () => {
    await updateConfig.mutateAsync({
      strategy_allocation: {
        shield_percent: shieldPercent,
        spear_percent: 100 - shieldPercent,
      },
    })
  }

  const handleResetCircuitBreaker = async () => {
    await resetCircuitBreaker.mutateAsync()
    setShowResetConfirm(false)
  }

  if (isLoading) {
    return (
      <div className="p-8 text-center text-text-muted">Loading configuration...</div>
    )
  }

  if (!isAdmin) {
    return (
      <div className="space-y-6">
        <Card>
          <CardContent>
            <div className="flex items-center gap-3 text-spear">
              <AlertTriangle className="w-5 h-5" />
              <span>Configuration management requires admin access.</span>
            </div>
          </CardContent>
        </Card>

        {/* Read-only view */}
        <ConfigReadOnly config={config} health={health} />
      </div>
    )
  }

  const circuitBreakerTripped = !health?.circuit_breaker.trading_allowed

  return (
    <div className="space-y-6">
      {/* Circuit Breaker Status Alert */}
      {circuitBreakerTripped && (
        <Card className="border-loss">
          <CardContent>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3 text-loss">
                <AlertTriangle className="w-5 h-5" />
                <div>
                  <div className="font-semibold">Circuit Breaker Tripped</div>
                  <div className="text-sm text-text-muted">
                    Trading is halted. Reason: {health?.circuit_breaker.trip_reason || 'Unknown'}
                  </div>
                </div>
              </div>
              <Button
                variant="danger"
                onClick={() => setShowResetConfirm(true)}
              >
                <RefreshCw className="w-4 h-4 mr-2" />
                Reset Circuit Breaker
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Circuit Breakers */}
      <Card>
        <CardHeader>
          <CardTitle>Circuit Breakers</CardTitle>
          <div className="flex items-center gap-2">
            {circuitBreakerTripped ? (
              <Badge variant="danger">Tripped</Badge>
            ) : (
              <Badge variant="success">Active</Badge>
            )}
          </div>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-2 gap-6">
            <div>
              <label className="block text-sm font-medium text-text mb-2">
                Max Loss (24h) - USD
              </label>
              <input
                type="number"
                value={maxLoss24h}
                onChange={(e) => setMaxLoss24h(parseFloat(e.target.value) || 0)}
                className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-text font-mono-numbers focus:outline-none focus:ring-2 focus:ring-shield"
              />
              <p className="text-xs text-text-muted mt-1">
                Halt trading if 24h losses exceed this amount
              </p>
            </div>

            <div>
              <label className="block text-sm font-medium text-text mb-2">
                Max Consecutive Losses
              </label>
              <input
                type="number"
                value={maxConsecutiveLosses}
                onChange={(e) => setMaxConsecutiveLosses(parseInt(e.target.value) || 0)}
                className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-text font-mono-numbers focus:outline-none focus:ring-2 focus:ring-shield"
              />
              <p className="text-xs text-text-muted mt-1">
                Pause Spear strategy after consecutive losses
              </p>
            </div>

            <div>
              <label className="block text-sm font-medium text-text mb-2">
                Max Drawdown (%)
              </label>
              <input
                type="number"
                step="0.1"
                value={maxDrawdown}
                onChange={(e) => setMaxDrawdown(parseFloat(e.target.value) || 0)}
                className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-text font-mono-numbers focus:outline-none focus:ring-2 focus:ring-shield"
              />
              <p className="text-xs text-text-muted mt-1">
                Emergency exit if portfolio drawdown exceeds this
              </p>
            </div>

            <div>
              <label className="block text-sm font-medium text-text mb-2">
                Cooldown Period (minutes)
              </label>
              <input
                type="number"
                value={cooldownMinutes}
                onChange={(e) => setCooldownMinutes(parseInt(e.target.value) || 0)}
                className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-text font-mono-numbers focus:outline-none focus:ring-2 focus:ring-shield"
              />
              <p className="text-xs text-text-muted mt-1">
                Wait time after circuit breaker trips
              </p>
            </div>
          </div>

          <div className="mt-6 flex justify-end">
            <Button
              variant="primary"
              onClick={handleSaveCircuitBreakers}
              loading={updateConfig.isPending}
              disabled={!hasChanges}
            >
              <Save className="w-4 h-4 mr-2" />
              Save Circuit Breaker Settings
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Strategy Allocation */}
      <Card>
        <CardHeader>
          <CardTitle>Strategy Allocation</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="space-y-4">
            <div>
              <div className="flex justify-between text-sm mb-2">
                <span>Shield: {shieldPercent}%</span>
                <span>Spear: {100 - shieldPercent}%</span>
              </div>
              <input
                type="range"
                min="0"
                max="100"
                value={shieldPercent}
                onChange={(e) => setShieldPercent(parseInt(e.target.value))}
                className="w-full h-2 bg-surface-light rounded-lg appearance-none cursor-pointer"
                style={{
                  background: `linear-gradient(to right, #00D4FF ${shieldPercent}%, #FF8800 ${shieldPercent}%)`,
                }}
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="bg-shield/10 border border-shield/30 rounded-lg p-4">
                <div className="text-shield font-semibold">🛡️ Shield</div>
                <div className="text-3xl font-mono-numbers mt-2">{shieldPercent}%</div>
                <div className="text-sm text-text-muted mt-1">Conservative</div>
              </div>
              <div className="bg-spear/10 border border-spear/30 rounded-lg p-4">
                <div className="text-spear font-semibold">⚔️ Spear</div>
                <div className="text-3xl font-mono-numbers mt-2">{100 - shieldPercent}%</div>
                <div className="text-sm text-text-muted mt-1">Aggressive</div>
              </div>
            </div>

            <div className="flex justify-end">
              <Button
                variant="primary"
                onClick={handleSaveAllocation}
                loading={updateConfig.isPending}
                disabled={config?.strategy_allocation.shield_percent === shieldPercent}
              >
                <Save className="w-4 h-4 mr-2" />
                Save Allocation
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* RPC Status */}
      <Card>
        <CardHeader>
          <CardTitle>RPC Status</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-3 gap-4">
            <div>
              <div className="text-sm text-text-muted">Primary</div>
              <div className="font-semibold capitalize">
                {config?.rpc_status.primary || 'Unknown'}
              </div>
            </div>
            <div>
              <div className="text-sm text-text-muted">Active</div>
              <div className="font-semibold capitalize">
                {config?.rpc_status.active || 'Unknown'}
              </div>
            </div>
            <div>
              <div className="text-sm text-text-muted">Fallback Active</div>
              <Badge
                variant={config?.rpc_status.fallback_triggered ? 'warning' : 'success'}
              >
                {config?.rpc_status.fallback_triggered ? 'Yes' : 'No'}
              </Badge>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Jito Tip Strategy */}
      <Card>
        <CardHeader>
          <CardTitle>Jito Tip Strategy</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-4 gap-4 text-sm">
            <div>
              <div className="text-text-muted">Tip Floor</div>
              <div className="font-mono-numbers">
                {config?.jito_tip_strategy.tip_floor.toFixed(4)} SOL
              </div>
            </div>
            <div>
              <div className="text-text-muted">Tip Ceiling</div>
              <div className="font-mono-numbers">
                {config?.jito_tip_strategy.tip_ceiling.toFixed(4)} SOL
              </div>
            </div>
            <div>
              <div className="text-text-muted">Percentile</div>
              <div className="font-mono-numbers">
                {config?.jito_tip_strategy.tip_percentile}th
              </div>
            </div>
            <div>
              <div className="text-text-muted">Max % of Trade</div>
              <div className="font-mono-numbers">
                {((config?.jito_tip_strategy.tip_percent_max || 0) * 100).toFixed(1)}%
              </div>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Reset Confirmation Modal */}
      <ConfirmModal
        isOpen={showResetConfirm}
        onClose={() => setShowResetConfirm(false)}
        onConfirm={handleResetCircuitBreaker}
        title="Reset Circuit Breaker"
        message="Are you sure you want to reset the circuit breaker? Trading will resume immediately."
        confirmLabel="Reset & Resume Trading"
        variant="danger"
        loading={resetCircuitBreaker.isPending}
      />
    </div>
  )
}

// Read-only view for non-admin users
function ConfigReadOnly({ config, health }: { config: any; health: any }) {
  if (!config) return null

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>Circuit Breakers</CardTitle>
          <Badge
            variant={health?.circuit_breaker.trading_allowed ? 'success' : 'danger'}
          >
            {health?.circuit_breaker.trading_allowed ? 'Active' : 'Tripped'}
          </Badge>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-4 gap-4 text-sm">
            <div>
              <div className="text-text-muted">Max Loss (24h)</div>
              <div className="font-mono-numbers">${config.circuit_breakers.max_loss_24h}</div>
            </div>
            <div>
              <div className="text-text-muted">Max Consecutive Losses</div>
              <div className="font-mono-numbers">{config.circuit_breakers.max_consecutive_losses}</div>
            </div>
            <div>
              <div className="text-text-muted">Max Drawdown</div>
              <div className="font-mono-numbers">{config.circuit_breakers.max_drawdown_percent}%</div>
            </div>
            <div>
              <div className="text-text-muted">Cooldown</div>
              <div className="font-mono-numbers">{config.circuit_breakers.cool_down_minutes} min</div>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Strategy Allocation</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-2 gap-4">
            <div className="bg-shield/10 border border-shield/30 rounded-lg p-4">
              <div className="text-shield font-semibold">🛡️ Shield</div>
              <div className="text-2xl font-mono-numbers mt-1">
                {config.strategy_allocation.shield_percent}%
              </div>
            </div>
            <div className="bg-spear/10 border border-spear/30 rounded-lg p-4">
              <div className="text-spear font-semibold">⚔️ Spear</div>
              <div className="text-2xl font-mono-numbers mt-1">
                {config.strategy_allocation.spear_percent}%
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
