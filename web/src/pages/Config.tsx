import { useState, useEffect } from 'react'
import { 
  AlertTriangle, Save, RefreshCw, History, Power, 
  Shield, Zap, TrendingUp, Target, Settings,
  Activity, Bell, ShieldCheck
} from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { Modal, ConfirmModal } from '../components/ui/Modal'
import { ConfigSection, ConfigInput, ConfigToggle, ConfigArrayInput } from '../components/config'
import { useConfig, useUpdateConfig, useResetCircuitBreaker, useHealth, useConfigAudit } from '../api'
import { useTripCircuitBreaker } from '../api/config'
import { useAuthStore } from '../stores/authStore'
import { toast } from '../components/ui/Toast'
import type { ConfigAudit, ConfigResponse, HealthResponse } from '../types'

export function Config() {
  const { hasPermission, user, isAuthenticated } = useAuthStore()
  const isAdmin = hasPermission('admin')

  const { data: config, isLoading, refetch } = useConfig()
  const { data: health } = useHealth()
  const updateConfig = useUpdateConfig()
  const resetCircuitBreaker = useResetCircuitBreaker()
  const tripCircuitBreaker = useTripCircuitBreaker()

  // Trading Configuration State
  const [maxLoss24h, setMaxLoss24h] = useState(0)
  const [maxConsecutiveLosses, setMaxConsecutiveLosses] = useState(0)
  const [maxDrawdown, setMaxDrawdown] = useState(0)
  const [cooldownMinutes, setCooldownMinutes] = useState(0)
  const [shieldPercent, setShieldPercent] = useState(70)
  const [maxPositionSol, setMaxPositionSol] = useState(1.0)
  const [minPositionSol, setMinPositionSol] = useState(0.01)

  // Profit Management State
  const [profitTargets, setProfitTargets] = useState<number[]>([])
  const [tieredExitPercent, setTieredExitPercent] = useState(25)
  const [trailingStopActivation, setTrailingStopActivation] = useState(50)
  const [trailingStopDistance, setTrailingStopDistance] = useState(20)
  const [hardStopLoss, setHardStopLoss] = useState(15)
  const [timeExitHours, setTimeExitHours] = useState(24)

  // Position Sizing State
  const [baseSizeSol, setBaseSizeSol] = useState(0.1)
  const [maxSizeSol, setMaxSizeSol] = useState(2.0)
  const [minSizeSol, setMinSizeSol] = useState(0.02)
  const [consensusMultiplier, setConsensusMultiplier] = useState(1.5)
  const [maxConcurrentPositions, setMaxConcurrentPositions] = useState(5)

  // MEV Protection State
  const [alwaysUseJito, setAlwaysUseJito] = useState(true)
  const [exitTipSol, setExitTipSol] = useState(0.007)
  const [consensusTipSol, setConsensusTipSol] = useState(0.003)
  const [standardTipSol, setStandardTipSol] = useState(0.0015)

  // Monitoring State
  const [monitoringEnabled, setMonitoringEnabled] = useState(false)
  const [webhookBatchSize, setWebhookBatchSize] = useState(10)
  const [webhookDelayMs, setWebhookDelayMs] = useState(200)
  const [webhookRateLimit, setWebhookRateLimit] = useState(45)
  const [rpcPollingEnabled, setRpcPollingEnabled] = useState(true)
  const [rpcPollInterval, setRpcPollInterval] = useState(8)
  const [rpcPollBatchSize, setRpcPollBatchSize] = useState(6)
  const [rpcPollRateLimit, setRpcPollRateLimit] = useState(40)
  const [maxActiveWallets, setMaxActiveWallets] = useState(20)

  // Token Safety State
  const [minLiquidityShield, setMinLiquidityShield] = useState(10000)
  const [minLiquiditySpear, setMinLiquiditySpear] = useState(5000)
  const [honeypotDetection, setHoneypotDetection] = useState(true)
  const [cacheCapacity, setCacheCapacity] = useState(1000)
  const [cacheTtl, setCacheTtl] = useState(3600)

  // Notifications State
  const [telegramEnabled, setTelegramEnabled] = useState(false)
  const [telegramRateLimit, setTelegramRateLimit] = useState(60)
  const [notifCircuitBreaker, setNotifCircuitBreaker] = useState(true)
  const [notifWalletDrained, setNotifWalletDrained] = useState(true)
  const [notifPositionExited, setNotifPositionExited] = useState(true)
  const [notifWalletPromoted, setNotifWalletPromoted] = useState(true)
  const [notifDailySummary, setNotifDailySummary] = useState(true)
  const [notifRpcFallback, setNotifRpcFallback] = useState(true)
  const [dailySummaryEnabled, setDailySummaryEnabled] = useState(true)
  const [dailySummaryHour, setDailySummaryHour] = useState(20)
  const [dailySummaryMinute, setDailySummaryMinute] = useState(0)

  // Queue State
  const [queueCapacity, setQueueCapacity] = useState(1000)
  const [loadShedThreshold, setLoadShedThreshold] = useState(80)

  // UI State
  const [showResetConfirm, setShowResetConfirm] = useState(false)
  const [showHistoryModal, setShowHistoryModal] = useState(false)
  const [showKillSwitchModal, setShowKillSwitchModal] = useState(false)
  const [killSwitchConfirm, setKillSwitchConfirm] = useState('')

  const { data: configAudit, isLoading: auditLoading } = useConfigAudit({ limit: 50 })

  // Initialize form from config
  useEffect(() => {
    if (config) {
      // Trading Configuration
      setMaxLoss24h(config.circuit_breakers?.max_loss_24h ?? 0)
      setMaxConsecutiveLosses(config.circuit_breakers?.max_consecutive_losses ?? 0)
      setMaxDrawdown(config.circuit_breakers?.max_drawdown_percent ?? 0)
      setCooldownMinutes(config.circuit_breakers?.cool_down_minutes ?? 0)
      setShieldPercent(config.strategy_allocation?.shield_percent ?? 70)
      setMaxPositionSol(config.strategy?.max_position_sol ?? 1.0)
      setMinPositionSol(config.strategy?.min_position_sol ?? 0.01)

      // Profit Management
      setProfitTargets(config.profit_management?.targets ?? [])
      setTieredExitPercent(config.profit_management?.tiered_exit_percent ?? 25)
      setTrailingStopActivation(config.profit_management?.trailing_stop_activation ?? 50)
      setTrailingStopDistance(config.profit_management?.trailing_stop_distance ?? 20)
      setHardStopLoss(config.profit_management?.hard_stop_loss ?? 15)
      setTimeExitHours(config.profit_management?.time_exit_hours ?? 24)

      // Position Sizing
      setBaseSizeSol(config.position_sizing?.base_size_sol ?? 0.1)
      setMaxSizeSol(config.position_sizing?.max_size_sol ?? 2.0)
      setMinSizeSol(config.position_sizing?.min_size_sol ?? 0.02)
      setConsensusMultiplier(config.position_sizing?.consensus_multiplier ?? 1.5)
      setMaxConcurrentPositions(config.position_sizing?.max_concurrent_positions ?? 5)

      // MEV Protection
      setAlwaysUseJito(config.mev_protection?.always_use_jito ?? true)
      setExitTipSol(config.mev_protection?.exit_tip_sol ?? 0.007)
      setConsensusTipSol(config.mev_protection?.consensus_tip_sol ?? 0.003)
      setStandardTipSol(config.mev_protection?.standard_tip_sol ?? 0.0015)

      // Monitoring
      if (config.monitoring) {
        setMonitoringEnabled(config.monitoring.enabled)
        setWebhookBatchSize(config.monitoring.webhook_registration_batch_size)
        setWebhookDelayMs(config.monitoring.webhook_registration_delay_ms)
        setWebhookRateLimit(config.monitoring.webhook_processing_rate_limit)
        setRpcPollingEnabled(config.monitoring.rpc_polling_enabled)
        setRpcPollInterval(config.monitoring.rpc_poll_interval_secs)
        setRpcPollBatchSize(config.monitoring.rpc_poll_batch_size)
        setRpcPollRateLimit(config.monitoring.rpc_poll_rate_limit)
        setMaxActiveWallets(config.monitoring.max_active_wallets)
      }

      // Token Safety
      setMinLiquidityShield(config.token_safety?.min_liquidity_shield_usd ?? 10000)
      setMinLiquiditySpear(config.token_safety?.min_liquidity_spear_usd ?? 5000)
      setHoneypotDetection(config.token_safety?.honeypot_detection_enabled ?? true)
      setCacheCapacity(config.token_safety?.cache_capacity ?? 1000)
      setCacheTtl(config.token_safety?.cache_ttl_seconds ?? 3600)

      // Notifications
      setTelegramEnabled(config.notifications?.telegram?.enabled ?? false)
      setTelegramRateLimit(config.notifications?.telegram?.rate_limit_seconds ?? 60)
      setNotifCircuitBreaker(config.notifications?.rules?.circuit_breaker_triggered ?? true)
      setNotifWalletDrained(config.notifications?.rules?.wallet_drained ?? true)
      setNotifPositionExited(config.notifications?.rules?.position_exited ?? true)
      setNotifWalletPromoted(config.notifications?.rules?.wallet_promoted ?? true)
      setNotifDailySummary(config.notifications?.rules?.daily_summary ?? true)
      setNotifRpcFallback(config.notifications?.rules?.rpc_fallback ?? true)
      setDailySummaryEnabled(config.notifications?.daily_summary?.enabled ?? true)
      setDailySummaryHour(config.notifications?.daily_summary?.hour_utc ?? 20)
      setDailySummaryMinute(config.notifications?.daily_summary?.minute ?? 0)

      // Queue
      setQueueCapacity(config.queue?.capacity ?? 1000)
      setLoadShedThreshold(config.queue?.load_shed_threshold_percent ?? 80)
    }
  }, [config])

  // Save handlers
  const handleSaveCircuitBreakers = async () => {
    try {
      await updateConfig.mutateAsync({
        circuit_breakers: {
          max_loss_24h: maxLoss24h,
          max_consecutive_losses: maxConsecutiveLosses,
          max_drawdown_percent: maxDrawdown,
          cool_down_minutes: cooldownMinutes,
        },
      })
      toast.success('Circuit breaker settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save circuit breaker settings')
    }
  }

  const handleSaveStrategy = async () => {
    try {
      await updateConfig.mutateAsync({
        strategy_allocation: {
          shield_percent: shieldPercent,
          spear_percent: 100 - shieldPercent,
        },
        strategy: {
          max_position_sol: maxPositionSol,
          min_position_sol: minPositionSol,
        },
      })
      toast.success('Strategy settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save strategy settings')
    }
  }

  const handleSaveProfitManagement = async () => {
    try {
      await updateConfig.mutateAsync({
        profit_management: {
          targets: profitTargets,
          tiered_exit_percent: tieredExitPercent,
          trailing_stop_activation: trailingStopActivation,
          trailing_stop_distance: trailingStopDistance,
          hard_stop_loss: hardStopLoss,
          time_exit_hours: timeExitHours,
        },
      })
      toast.success('Profit management settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save profit management settings')
    }
  }

  const handleSavePositionSizing = async () => {
    try {
      if (minSizeSol >= baseSizeSol || baseSizeSol >= maxSizeSol) {
        toast.error('Position sizes must be: min < base < max')
        return
      }
      await updateConfig.mutateAsync({
        position_sizing: {
          base_size_sol: baseSizeSol,
          max_size_sol: maxSizeSol,
          min_size_sol: minSizeSol,
          consensus_multiplier: consensusMultiplier,
          max_concurrent_positions: maxConcurrentPositions,
        },
      })
      toast.success('Position sizing settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save position sizing settings')
    }
  }

  const handleSaveMevProtection = async () => {
    try {
      await updateConfig.mutateAsync({
        mev_protection: {
          always_use_jito: alwaysUseJito,
          exit_tip_sol: exitTipSol,
          consensus_tip_sol: consensusTipSol,
          standard_tip_sol: standardTipSol,
        },
      })
      toast.success('MEV protection settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save MEV protection settings')
    }
  }

  const handleSaveMonitoring = async () => {
    try {
      if (webhookRateLimit > 50 || rpcPollRateLimit > 50) {
        toast.error('Rate limits cannot exceed 50 req/sec (Helius limit)')
        return
      }
      await updateConfig.mutateAsync({
        monitoring: {
          enabled: monitoringEnabled,
          webhook_registration_batch_size: webhookBatchSize,
          webhook_registration_delay_ms: webhookDelayMs,
          webhook_processing_rate_limit: webhookRateLimit,
          rpc_polling_enabled: rpcPollingEnabled,
          rpc_poll_interval_secs: rpcPollInterval,
          rpc_poll_batch_size: rpcPollBatchSize,
          rpc_poll_rate_limit: rpcPollRateLimit,
          max_active_wallets: maxActiveWallets,
        },
      })
      toast.success('Monitoring settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save monitoring settings')
    }
  }

  const handleSaveTokenSafety = async () => {
    try {
      await updateConfig.mutateAsync({
        token_safety: {
          min_liquidity_shield_usd: minLiquidityShield,
          min_liquidity_spear_usd: minLiquiditySpear,
          honeypot_detection_enabled: honeypotDetection,
          cache_capacity: cacheCapacity,
          cache_ttl_seconds: cacheTtl,
        },
      })
      toast.success('Token safety settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save token safety settings')
    }
  }

  const handleSaveNotifications = async () => {
    try {
      await updateConfig.mutateAsync({
        notifications: {
          telegram: {
            enabled: telegramEnabled,
            rate_limit_seconds: telegramRateLimit,
          },
          rules: {
            circuit_breaker_triggered: notifCircuitBreaker,
            wallet_drained: notifWalletDrained,
            position_exited: notifPositionExited,
            wallet_promoted: notifWalletPromoted,
            daily_summary: notifDailySummary,
            rpc_fallback: notifRpcFallback,
          },
          daily_summary: {
            enabled: dailySummaryEnabled,
            hour_utc: dailySummaryHour,
            minute: dailySummaryMinute,
          },
        },
      })
      toast.success('Notification settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save notification settings')
    }
  }

  const handleSaveQueue = async () => {
    try {
      await updateConfig.mutateAsync({
        queue: {
          capacity: queueCapacity,
          load_shed_threshold_percent: loadShedThreshold,
        },
      })
      toast.success('Queue settings saved')
      refetch()
    } catch (error) {
      toast.error('Failed to save queue settings')
    }
  }

  const handleResetCircuitBreaker = async () => {
    try {
      await resetCircuitBreaker.mutateAsync()
      setShowResetConfirm(false)
      toast.success('Circuit breaker reset successfully')
    } catch (error) {
      toast.error('Failed to reset circuit breaker')
    }
  }

  const handleEmergencyKillSwitch = async () => {
    if (killSwitchConfirm !== 'HALT') {
      toast.error('Please type "HALT" to confirm')
      return
    }
    
    // Verify we have authentication before proceeding
    const { user, isAuthenticated } = useAuthStore.getState()
    if (!isAuthenticated || !user?.token) {
      toast.error('You must be authenticated to activate the kill switch. Please log in again.')
      return
    }
    
    // Debug: Log the token being used (first 8 chars only for security)
    console.log('Using token:', user.token.substring(0, 8) + '...', 'Type:', user.token.includes('.') ? 'JWT' : 'Wallet Address')
    
    try {
      // Use the dedicated trip endpoint which immediately halts trading
      await tripCircuitBreaker.mutateAsync('Emergency kill switch activated')
      setShowKillSwitchModal(false)
      setKillSwitchConfirm('')
      toast.success('Emergency kill switch activated. All trading halted.')
      // Refetch health to show updated status
      setTimeout(() => {
        window.location.reload()
      }, 1000)
    } catch (error: any) {
      const errorMessage = error.response?.data?.details || error.response?.data?.reason || error.message
      if (error.response?.status === 401 || error.response?.status === 403) {
        toast.error(`Authentication failed: ${errorMessage}. Please log in again with your admin wallet.`)
        // Don't auto-logout - let user try to re-authenticate
      } else {
        toast.error(`Failed to activate kill switch: ${errorMessage}`)
      }
    }
  }

  // Calculate secret rotation status
  const getSecretRotationStatus = () => {
    if (!configAudit?.items) return null
    const webhookRotations = configAudit.items.filter(
      (item) => item.key.includes('secret_rotation.webhook')
    )
    const rpcRotations = configAudit.items.filter(
      (item) => item.key.includes('secret_rotation.rpc')
    )
    const lastWebhookRotation = webhookRotations[0]
    const lastRpcRotation = rpcRotations[0]
    const getNextRotationDate = (lastDate: string, days: number) => {
      const date = new Date(lastDate)
      date.setDate(date.getDate() + days)
      return date
    }
    return {
      webhook: lastWebhookRotation
        ? {
            lastRotated: new Date(lastWebhookRotation.changed_at),
            nextDue: getNextRotationDate(lastWebhookRotation.changed_at, 30),
            daysUntilDue: Math.ceil(
              (getNextRotationDate(lastWebhookRotation.changed_at, 30).getTime() -
                new Date().getTime()) /
                (1000 * 60 * 60 * 24)
            ),
          }
        : null,
      rpc: lastRpcRotation
        ? {
            lastRotated: new Date(lastRpcRotation.changed_at),
            nextDue: getNextRotationDate(lastRpcRotation.changed_at, 90),
            daysUntilDue: Math.ceil(
              (getNextRotationDate(lastRpcRotation.changed_at, 90).getTime() -
                new Date().getTime()) /
                (1000 * 60 * 60 * 24)
            ),
          }
        : null,
    }
  }

  const rotationStatus = getSecretRotationStatus()

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
        <ConfigReadOnly config={config || null} health={health || null} />
      </div>
    )
  }

  const circuitBreakerTripped = !health?.circuit_breaker?.trading_allowed

  return (
    <div className="space-y-6 pb-20 md:pb-6">
      {/* Circuit Breaker Status Alert */}
      {circuitBreakerTripped && (
        <Card className="border-loss">
          <CardContent>
            <div className="flex flex-col sm:flex-row items-stretch sm:items-center justify-between gap-3">
              <div className="flex items-start gap-3 text-loss">
                <AlertTriangle className="w-5 h-5 flex-shrink-0 mt-0.5" />
                <div className="min-w-0">
                  <div className="font-semibold text-sm md:text-base">Circuit Breaker Tripped</div>
                  <div className="text-xs md:text-sm text-text-muted break-words">
                    Trading is halted. Reason: {health?.circuit_breaker?.trip_reason || 'Unknown'}
                  </div>
                </div>
              </div>
              <Button
                variant="danger"
                onClick={() => setShowResetConfirm(true)}
                className="w-full sm:w-auto"
                size="sm"
              >
                <RefreshCw className="w-4 h-4 mr-2" />
                <span className="hidden sm:inline">Reset </span>Circuit Breaker
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Trading Configuration */}
      <ConfigSection
        title="Trading Configuration"
        description="Core trading parameters and risk management"
        defaultOpen={true}
        collapsible={false}
        icon={<Shield className="w-5 h-5" />}
      >
        <div className="space-y-6">
          {/* Circuit Breakers */}
          <div>
            <div className="flex items-center gap-2 mb-4">
              <h4 className="text-sm font-semibold text-text">Circuit Breakers</h4>
              {circuitBreakerTripped ? (
                <Badge variant="danger">Tripped</Badge>
              ) : (
                <Badge variant="success">Active</Badge>
              )}
            </div>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <ConfigInput
                label="Max Loss (24h)"
                description="Halt trading if 24h losses exceed this amount"
                value={maxLoss24h}
                onChange={setMaxLoss24h}
                type="number"
                min={0}
                disabled={!isAdmin}
                unit="USD"
              />
              <ConfigInput
                label="Max Consecutive Losses"
                description="Pause Spear strategy after consecutive losses"
                value={maxConsecutiveLosses}
                onChange={setMaxConsecutiveLosses}
                type="number"
                min={1}
                disabled={!isAdmin}
              />
              <ConfigInput
                label="Max Drawdown"
                description="Emergency exit if portfolio drawdown exceeds this"
                value={maxDrawdown}
                onChange={setMaxDrawdown}
                type="number"
                min={0}
                max={100}
                step={0.1}
                disabled={!isAdmin}
                unit="%"
              />
              <ConfigInput
                label="Cooldown Period"
                description="Wait time after circuit breaker trips"
                value={cooldownMinutes}
                onChange={setCooldownMinutes}
                type="number"
                min={0}
                disabled={!isAdmin}
                unit="minutes"
              />
            </div>
            <div className="mt-4 flex justify-end">
              <Button
                variant="primary"
                onClick={handleSaveCircuitBreakers}
                loading={updateConfig.isPending}
              >
                <Save className="w-4 h-4 mr-2" />
                Save Circuit Breakers
              </Button>
            </div>
          </div>

          {/* Strategy Allocation */}
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Strategy Allocation</h4>
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
                  disabled={!isAdmin}
                  className="w-full h-2 bg-surface-light rounded-lg appearance-none cursor-pointer disabled:opacity-50"
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
            </div>
          </div>

          {/* Position Limits */}
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Position Limits</h4>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <ConfigInput
                label="Max Position Size"
                description="Maximum position size per trade"
                value={maxPositionSol}
                onChange={(v: string | number) => setMaxPositionSol(typeof v === 'number' ? v : parseFloat(String(v)) || 0)}
                type="number"
                min={0.01}
                step={0.1}
                disabled={!isAdmin}
                unit="SOL"
              />
              <ConfigInput
                label="Min Position Size"
                description="Minimum position size per trade"
                value={minPositionSol}
                onChange={(v: string | number) => setMinPositionSol(typeof v === 'number' ? v : parseFloat(String(v)) || 0)}
                type="number"
                min={0.001}
                step={0.01}
                disabled={!isAdmin}
                unit="SOL"
              />
            </div>
            <div className="mt-4 flex justify-end">
              <Button
                variant="primary"
                onClick={handleSaveStrategy}
                loading={updateConfig.isPending}
              >
                <Save className="w-4 h-4 mr-2" />
                Save Strategy Settings
              </Button>
            </div>
          </div>
        </div>
      </ConfigSection>

      {/* Profit Management */}
      <ConfigSection
        title="Profit Management"
        description="Profit targets, trailing stops, and exit strategies"
        defaultOpen={false}
        icon={<Target className="w-5 h-5" />}
        badge={profitTargets.length > 0 ? <Badge variant="info">{profitTargets.length} targets</Badge> : undefined}
      >
        <div className="space-y-6">
          <ConfigArrayInput
            label="Profit Targets"
            description="Percentage targets for tiered exits (e.g., 25%, 50%, 100%, 200%)"
            values={profitTargets}
            onChange={setProfitTargets}
            disabled={!isAdmin}
            min={0}
            max={1000}
            unit="%"
          />
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <ConfigInput
              label="Tiered Exit Percentage"
              description="Percentage to sell at each target"
              value={tieredExitPercent}
              onChange={setTieredExitPercent}
              type="number"
              min={0}
              max={100}
              disabled={!isAdmin}
              unit="%"
            />
            <ConfigInput
              label="Trailing Stop Activation"
              description="Activate trailing stop after this profit %"
              value={trailingStopActivation}
              onChange={setTrailingStopActivation}
              type="number"
              min={0}
              max={500}
              disabled={!isAdmin}
              unit="%"
            />
            <ConfigInput
              label="Trailing Stop Distance"
              description="Distance from peak for trailing stop"
              value={trailingStopDistance}
              onChange={setTrailingStopDistance}
              type="number"
              min={0}
              max={100}
              disabled={!isAdmin}
              unit="%"
            />
            <ConfigInput
              label="Hard Stop Loss"
              description="Maximum loss before forced exit"
              value={hardStopLoss}
              onChange={setHardStopLoss}
              type="number"
              min={0}
              max={100}
              disabled={!isAdmin}
              unit="%"
            />
            <ConfigInput
              label="Time-based Exit"
              description="Auto-exit profitable positions after this time"
              value={timeExitHours}
              onChange={setTimeExitHours}
              type="number"
              min={0}
              max={168}
              disabled={!isAdmin}
              unit="hours"
            />
          </div>
          <div className="flex justify-end">
            <Button
              variant="primary"
              onClick={handleSaveProfitManagement}
              loading={updateConfig.isPending}
            >
              <Save className="w-4 h-4 mr-2" />
              Save Profit Management
            </Button>
          </div>
        </div>
      </ConfigSection>

      {/* Position Sizing */}
      <ConfigSection
        title="Position Sizing"
        description="Dynamic position sizing based on confidence and wallet performance"
        defaultOpen={false}
        icon={<TrendingUp className="w-5 h-5" />}
      >
        <div className="space-y-6">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <ConfigInput
              label="Base Size"
              description="Base position size before multipliers"
              value={baseSizeSol}
              onChange={setBaseSizeSol}
              type="number"
              min={0.001}
              step={0.01}
              disabled={!isAdmin}
              unit="SOL"
            />
            <ConfigInput
              label="Max Size"
              description="Maximum position size (after multipliers)"
              value={maxSizeSol}
              onChange={setMaxSizeSol}
              type="number"
              min={baseSizeSol}
              step={0.1}
              disabled={!isAdmin}
              unit="SOL"
            />
            <ConfigInput
              label="Min Size"
              description="Minimum position size (after multipliers)"
              value={minSizeSol}
              onChange={setMinSizeSol}
              type="number"
              min={0.001}
              max={baseSizeSol}
              step={0.01}
              disabled={!isAdmin}
              unit="SOL"
            />
            <ConfigInput
              label="Consensus Multiplier"
              description="Multiplier when multiple wallets buy same token"
              value={consensusMultiplier}
              onChange={setConsensusMultiplier}
              type="number"
              min={1.0}
              max={5.0}
              step={0.1}
              disabled={!isAdmin}
            />
            <ConfigInput
              label="Max Concurrent Positions"
              description="Maximum number of open positions"
              value={maxConcurrentPositions}
              onChange={setMaxConcurrentPositions}
              type="number"
              min={1}
              max={20}
              disabled={!isAdmin}
            />
          </div>
          <div className="flex justify-end">
            <Button
              variant="primary"
              onClick={handleSavePositionSizing}
              loading={updateConfig.isPending}
            >
              <Save className="w-4 h-4 mr-2" />
              Save Position Sizing
            </Button>
          </div>
        </div>
      </ConfigSection>

      {/* MEV Protection */}
      <ConfigSection
        title="MEV Protection"
        description="Jito bundle settings and tip strategies"
        defaultOpen={false}
        icon={<Zap className="w-5 h-5" />}
        badge={alwaysUseJito ? <Badge variant="success">Enabled</Badge> : <Badge variant="default">Disabled</Badge>}
      >
        <div className="space-y-6">
          <ConfigToggle
            label="Always Use Jito Bundles"
            description="Use Jito bundles for all trades (not just Spear)"
            enabled={alwaysUseJito}
            onChange={setAlwaysUseJito}
            disabled={!isAdmin}
          />
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <ConfigInput
              label="Exit Tip"
              description="Tip for exit signals (highest priority)"
              value={exitTipSol}
              onChange={setExitTipSol}
              type="number"
              min={0.001}
              max={0.1}
              step={0.001}
              disabled={!isAdmin}
              unit="SOL"
            />
            <ConfigInput
              label="Consensus Tip"
              description="Tip for consensus signals (multiple wallets)"
              value={consensusTipSol}
              onChange={setConsensusTipSol}
              type="number"
              min={0.001}
              max={0.1}
              step={0.001}
              disabled={!isAdmin}
              unit="SOL"
            />
            <ConfigInput
              label="Standard Tip"
              description="Tip for standard signals"
              value={standardTipSol}
              onChange={setStandardTipSol}
              type="number"
              min={0.001}
              max={0.1}
              step={0.001}
              disabled={!isAdmin}
              unit="SOL"
            />
          </div>
          <div className="flex justify-end">
            <Button
              variant="primary"
              onClick={handleSaveMevProtection}
              loading={updateConfig.isPending}
            >
              <Save className="w-4 h-4 mr-2" />
              Save MEV Protection
            </Button>
          </div>
        </div>
      </ConfigSection>

      {/* Monitoring */}
      <ConfigSection
        title="Monitoring"
        description="Automatic wallet monitoring via Helius webhooks and RPC polling"
        defaultOpen={false}
        icon={<Activity className="w-5 h-5" />}
        badge={monitoringEnabled ? <Badge variant="success">Active</Badge> : <Badge variant="default">Inactive</Badge>}
      >
        <div className="space-y-6">
          <ConfigToggle
            label="Enable Monitoring"
            description="Enable automatic on-chain transaction monitoring"
            enabled={monitoringEnabled}
            onChange={setMonitoringEnabled}
            disabled={!isAdmin}
          />
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Webhook Settings</h4>
            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <ConfigInput
                label="Registration Batch Size"
                description="Number of wallets per batch"
                value={webhookBatchSize}
                onChange={setWebhookBatchSize}
                type="number"
                min={1}
                max={20}
                disabled={!isAdmin}
              />
              <ConfigInput
                label="Registration Delay"
                description="Delay between batches"
                value={webhookDelayMs}
                onChange={setWebhookDelayMs}
                type="number"
                min={0}
                max={1000}
                disabled={!isAdmin}
                unit="ms"
              />
              <ConfigInput
                label="Processing Rate Limit"
                description="Max requests per second (max 50)"
                value={webhookRateLimit}
                onChange={setWebhookRateLimit}
                type="number"
                min={1}
                max={50}
                disabled={!isAdmin}
                unit="req/sec"
              />
            </div>
          </div>
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">RPC Polling Settings</h4>
            <ConfigToggle
              label="Enable RPC Polling"
              description="Use RPC polling as fallback when webhooks fail"
              enabled={rpcPollingEnabled}
              onChange={setRpcPollingEnabled}
              disabled={!isAdmin}
            />
            <div className="grid grid-cols-1 md:grid-cols-4 gap-4 mt-4">
              <ConfigInput
                label="Poll Interval"
                description="Interval between polls"
                value={rpcPollInterval}
                onChange={setRpcPollInterval}
                type="number"
                min={1}
                max={60}
                disabled={!isAdmin}
                unit="sec"
              />
              <ConfigInput
                label="Poll Batch Size"
                description="Wallets per polling batch"
                value={rpcPollBatchSize}
                onChange={setRpcPollBatchSize}
                type="number"
                min={1}
                max={20}
                disabled={!isAdmin}
              />
              <ConfigInput
                label="Poll Rate Limit"
                description="Max requests per second (max 50)"
                value={rpcPollRateLimit}
                onChange={setRpcPollRateLimit}
                type="number"
                min={1}
                max={50}
                disabled={!isAdmin}
                unit="req/sec"
              />
              <ConfigInput
                label="Max Active Wallets"
                description="Maximum wallets to monitor simultaneously"
                value={maxActiveWallets}
                onChange={setMaxActiveWallets}
                type="number"
                min={1}
                max={100}
                disabled={!isAdmin}
              />
            </div>
          </div>
          <div className="flex justify-end">
            <Button
              variant="primary"
              onClick={handleSaveMonitoring}
              loading={updateConfig.isPending}
            >
              <Save className="w-4 h-4 mr-2" />
              Save Monitoring Settings
            </Button>
          </div>
        </div>
      </ConfigSection>

      {/* Token Safety */}
      <ConfigSection
        title="Token Safety"
        description="Liquidity thresholds, honeypot detection, and token validation"
        defaultOpen={false}
        icon={<ShieldCheck className="w-5 h-5" />}
        badge={honeypotDetection ? <Badge variant="success">Protected</Badge> : <Badge variant="warning">Unprotected</Badge>}
      >
        <div className="space-y-6">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <ConfigInput
              label="Min Liquidity (Shield)"
              description="Minimum liquidity for Shield strategy trades"
              value={minLiquidityShield}
              onChange={setMinLiquidityShield}
              type="number"
              min={0}
              step={1000}
              disabled={!isAdmin}
              unit="USD"
            />
            <ConfigInput
              label="Min Liquidity (Spear)"
              description="Minimum liquidity for Spear strategy trades"
              value={minLiquiditySpear}
              onChange={setMinLiquiditySpear}
              type="number"
              min={0}
              step={1000}
              disabled={!isAdmin}
              unit="USD"
            />
          </div>
          <ConfigToggle
            label="Honeypot Detection"
            description="Enable transaction simulation to detect honeypots"
            enabled={honeypotDetection}
            onChange={setHoneypotDetection}
            disabled={!isAdmin}
          />
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <ConfigInput
              label="Token Cache Capacity"
              description="Maximum cached tokens"
              value={cacheCapacity}
              onChange={setCacheCapacity}
              type="number"
              min={100}
              max={10000}
              step={100}
              disabled={!isAdmin}
            />
            <ConfigInput
              label="Cache TTL"
              description="Time to live for cached token data"
                value={cacheTtl}
                onChange={(v: string | number) => setCacheTtl(typeof v === 'number' ? v : parseInt(String(v)) || 0)}
              type="number"
              min={60}
              max={86400}
              step={60}
              disabled={!isAdmin}
              unit="seconds"
            />
          </div>
          {config && config.token_safety && (
            <div>
              <h4 className="text-sm font-semibold text-text mb-2">Authority Whitelists (Read-only)</h4>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-sm">
                <div>
                  <div className="text-text-muted mb-2">Freeze Authority Whitelist</div>
                  <div className="bg-surface-light rounded p-2 font-mono text-xs">
                    {config.token_safety.freeze_authority_whitelist?.length ?? 0} tokens
                  </div>
                </div>
                <div>
                  <div className="text-text-muted mb-2">Mint Authority Whitelist</div>
                  <div className="bg-surface-light rounded p-2 font-mono text-xs">
                    {config.token_safety.mint_authority_whitelist?.length ?? 0} tokens
                  </div>
                </div>
              </div>
            </div>
          )}
          <div className="flex justify-end">
            <Button
              variant="primary"
              onClick={handleSaveTokenSafety}
              loading={updateConfig.isPending}
            >
              <Save className="w-4 h-4 mr-2" />
              Save Token Safety
            </Button>
          </div>
        </div>
      </ConfigSection>

      {/* Notifications */}
      <ConfigSection
        title="Notifications"
        description="Telegram notifications and alert rules"
        defaultOpen={false}
        icon={<Bell className="w-5 h-5" />}
        badge={telegramEnabled ? <Badge variant="success">Enabled</Badge> : <Badge variant="default">Disabled</Badge>}
      >
        <div className="space-y-6">
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Telegram Settings</h4>
            <ConfigToggle
              label="Enable Telegram Notifications"
              description="Send notifications via Telegram bot"
              enabled={telegramEnabled}
              onChange={setTelegramEnabled}
              disabled={!isAdmin}
            />
            <div className="mt-4">
              <ConfigInput
                label="Rate Limit"
                description="Minimum seconds between similar notifications"
                value={telegramRateLimit}
                onChange={(v: string | number) => setTelegramRateLimit(typeof v === 'number' ? v : parseInt(String(v)) || 0)}
                type="number"
                min={0}
                max={3600}
                disabled={!isAdmin}
                unit="seconds"
              />
            </div>
          </div>
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Notification Rules</h4>
            <div className="space-y-3">
              <ConfigToggle
                label="Circuit Breaker Triggered"
                enabled={notifCircuitBreaker}
                onChange={setNotifCircuitBreaker}
                disabled={!isAdmin}
              />
              <ConfigToggle
                label="Wallet Drained"
                enabled={notifWalletDrained}
                onChange={setNotifWalletDrained}
                disabled={!isAdmin}
              />
              <ConfigToggle
                label="Position Exited"
                enabled={notifPositionExited}
                onChange={setNotifPositionExited}
                disabled={!isAdmin}
              />
              <ConfigToggle
                label="Wallet Promoted"
                enabled={notifWalletPromoted}
                onChange={setNotifWalletPromoted}
                disabled={!isAdmin}
              />
              <ConfigToggle
                label="Daily Summary"
                enabled={notifDailySummary}
                onChange={setNotifDailySummary}
                disabled={!isAdmin}
              />
              <ConfigToggle
                label="RPC Fallback"
                enabled={notifRpcFallback}
                onChange={setNotifRpcFallback}
                disabled={!isAdmin}
              />
            </div>
          </div>
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Daily Summary Schedule</h4>
            <ConfigToggle
              label="Enable Daily Summary"
              enabled={dailySummaryEnabled}
              onChange={setDailySummaryEnabled}
              disabled={!isAdmin}
            />
            <div className="grid grid-cols-2 gap-4 mt-4">
              <ConfigInput
                label="Hour (UTC)"
                description="Hour of day to send summary"
                value={dailySummaryHour}
                onChange={(v: string | number) => {
                  const num = typeof v === 'number' ? v : parseInt(String(v)) || 0
                  setDailySummaryHour(num)
                }}
                type="number"
                min={0}
                max={23}
                disabled={!isAdmin}
              />
              <ConfigInput
                label="Minute"
                description="Minute of hour to send summary"
                value={dailySummaryMinute}
                onChange={(v: string | number) => {
                  const num = typeof v === 'number' ? v : parseInt(String(v)) || 0
                  setDailySummaryMinute(num)
                }}
                type="number"
                min={0}
                max={59}
                disabled={!isAdmin}
              />
            </div>
          </div>
          <div className="flex justify-end">
            <Button
              variant="primary"
              onClick={handleSaveNotifications}
              loading={updateConfig.isPending}
            >
              <Save className="w-4 h-4 mr-2" />
              Save Notifications
            </Button>
          </div>
        </div>
      </ConfigSection>

      {/* System Settings */}
      <ConfigSection
        title="System Settings"
        description="RPC status, Jito tips, queue settings, and secret rotation"
        defaultOpen={true}
        collapsible={false}
        icon={<Settings className="w-5 h-5" />}
      >
        <div className="space-y-6">
          {/* RPC Status */}
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">RPC Status</h4>
            <div className="grid grid-cols-2 md:grid-cols-3 gap-4">
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
          </div>

          {/* Jito Tip Strategy */}
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Jito Tip Strategy</h4>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
              <div>
                <div className="text-text-muted">Tip Floor</div>
                <div className="font-mono-numbers">
                  {config?.jito_tip_strategy?.tip_floor.toFixed(4) ?? '0.0000'} SOL
                </div>
              </div>
              <div>
                <div className="text-text-muted">Tip Ceiling</div>
                <div className="font-mono-numbers">
                  {config?.jito_tip_strategy?.tip_ceiling.toFixed(4) ?? '0.0000'} SOL
                </div>
              </div>
              <div>
                <div className="text-text-muted">Percentile</div>
                <div className="font-mono-numbers">
                  {config?.jito_tip_strategy?.tip_percentile ?? 50}th
                </div>
              </div>
              <div>
                <div className="text-text-muted">Max % of Trade</div>
                <div className="font-mono-numbers">
                  {((config?.jito_tip_strategy?.tip_percent_max || 0) * 100).toFixed(1)}%
                </div>
              </div>
            </div>
          </div>

          {/* Queue Settings */}
          <div>
            <h4 className="text-sm font-semibold text-text mb-4">Queue Settings</h4>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <ConfigInput
                label="Queue Capacity"
                description="Maximum number of queued signals"
                value={queueCapacity}
                onChange={setQueueCapacity}
                type="number"
                min={100}
                max={10000}
                step={100}
                disabled={!isAdmin}
              />
              <ConfigInput
                label="Load Shed Threshold"
                description="Percentage at which to start dropping low-priority signals"
                value={loadShedThreshold}
                onChange={setLoadShedThreshold}
                type="number"
                min={0}
                max={100}
                disabled={!isAdmin}
                unit="%"
              />
            </div>
            <div className="mt-4 flex justify-end">
              <Button
                variant="primary"
                onClick={handleSaveQueue}
                loading={updateConfig.isPending}
              >
                <Save className="w-4 h-4 mr-2" />
                Save Queue Settings
              </Button>
            </div>
          </div>

          {/* Secret Rotation Status */}
          <div>
            <div className="flex items-center justify-between mb-4">
              <h4 className="text-sm font-semibold text-text">Secret Rotation Status</h4>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setShowHistoryModal(true)}
              >
                <History className="w-4 h-4 mr-2" />
                View History
              </Button>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
              <div>
                <div className="text-sm font-semibold mb-3">Webhook HMAC Key</div>
                {rotationStatus?.webhook ? (
                  <div className="space-y-2 text-sm">
                    <div className="flex justify-between">
                      <span className="text-text-muted">Last Rotated:</span>
                      <span>{rotationStatus.webhook.lastRotated.toLocaleDateString()}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-text-muted">Next Due:</span>
                      <span>{rotationStatus.webhook.nextDue.toLocaleDateString()}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-text-muted">Days Until Due:</span>
                      <Badge
                        variant={
                          rotationStatus.webhook.daysUntilDue <= 7
                            ? 'warning'
                            : rotationStatus.webhook.daysUntilDue <= 0
                            ? 'danger'
                            : 'success'
                        }
                      >
                        {rotationStatus.webhook.daysUntilDue} days
                      </Badge>
                    </div>
                  </div>
                ) : (
                  <div className="text-sm text-text-muted">No rotation history</div>
                )}
              </div>
              <div>
                <div className="text-sm font-semibold mb-3">RPC API Keys</div>
                {rotationStatus?.rpc ? (
                  <div className="space-y-2 text-sm">
                    <div className="flex justify-between">
                      <span className="text-text-muted">Last Rotated:</span>
                      <span>{rotationStatus.rpc.lastRotated.toLocaleDateString()}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-text-muted">Next Due:</span>
                      <span>{rotationStatus.rpc.nextDue.toLocaleDateString()}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-text-muted">Days Until Due:</span>
                      <Badge
                        variant={
                          rotationStatus.rpc.daysUntilDue <= 14
                            ? 'warning'
                            : rotationStatus.rpc.daysUntilDue <= 0
                            ? 'danger'
                            : 'success'
                        }
                      >
                        {rotationStatus.rpc.daysUntilDue} days
                      </Badge>
                    </div>
                  </div>
                ) : (
                  <div className="text-sm text-text-muted">No rotation history</div>
                )}
              </div>
            </div>
          </div>
        </div>
      </ConfigSection>

      {/* Emergency Kill Switch */}
      <Card className="border-loss">
        <CardHeader>
          <CardTitle className="text-loss">Emergency Kill Switch</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex flex-col sm:flex-row items-stretch sm:items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="font-semibold text-loss mb-1 text-sm md:text-base">Halt All Trading</div>
              <div className="text-xs md:text-sm text-text-muted">
                Immediately stop all trading activity. Requires confirmation.
              </div>
            </div>
            <Button
              variant="danger"
              onClick={() => setShowKillSwitchModal(true)}
              className="w-full sm:w-auto"
              size="sm"
            >
              <Power className="w-4 h-4 mr-2" />
              <span className="hidden sm:inline">Activate </span>Kill Switch
            </Button>
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

      {/* Change History Modal */}
      <Modal
        isOpen={showHistoryModal}
        onClose={() => setShowHistoryModal(false)}
        title="Configuration Change History"
        size="lg"
      >
        {auditLoading ? (
          <div className="p-8 text-center text-text-muted">Loading history...</div>
        ) : configAudit?.items && configAudit.items.length > 0 ? (
          <div className="max-h-96 overflow-y-auto">
            <table className="w-full text-sm">
              <thead className="sticky top-0 bg-surface border-b border-border">
                <tr>
                  <th className="text-left p-2 font-semibold">Key</th>
                  <th className="text-left p-2 font-semibold">Changed By</th>
                  <th className="text-left p-2 font-semibold">Date</th>
                  <th className="text-left p-2 font-semibold">Reason</th>
                </tr>
              </thead>
              <tbody>
                {configAudit.items.map((item: ConfigAudit) => (
                  <tr key={item.id} className="border-b border-border hover:bg-surface-light">
                    <td className="p-2 font-mono text-xs">{item.key}</td>
                    <td className="p-2">
                      <Badge variant="default" size="sm">
                        {item.changed_by}
                      </Badge>
                    </td>
                    <td className="p-2 text-text-muted">
                      {new Date(item.changed_at).toLocaleString()}
                    </td>
                    <td className="p-2 text-text-muted text-xs">
                      {item.change_reason || '-'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="p-8 text-center text-text-muted">No change history found</div>
        )}
      </Modal>

      {/* Emergency Kill Switch Modal */}
      <Modal
        isOpen={showKillSwitchModal}
        onClose={() => {
          setShowKillSwitchModal(false)
          setKillSwitchConfirm('')
        }}
        title="Emergency Kill Switch"
        size="sm"
      >
        <div className="space-y-4">
          <div className="flex items-start gap-3 text-loss">
            <AlertTriangle className="w-5 h-5 mt-0.5" />
            <div>
              <div className="font-semibold mb-1">Warning: This will halt all trading</div>
              <div className="text-sm text-text-muted">
                All circuit breakers will be set to extreme values to immediately stop all trading activity. 
                This action can only be reversed by manually resetting the circuit breaker.
              </div>
            </div>
          </div>
          <div className="bg-surface-light border border-loss/20 rounded-lg p-3">
            <label className="block text-sm font-medium text-text mb-2">
              Type <span className="font-mono font-semibold text-loss">HALT</span> to confirm:
            </label>
            <input
              type="text"
              value={killSwitchConfirm}
              onChange={(e) => setKillSwitchConfirm(e.target.value)}
              className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-text focus:outline-none focus:ring-2 focus:ring-loss font-mono"
              placeholder="Type HALT here"
              autoComplete="off"
            />
            {killSwitchConfirm && killSwitchConfirm !== 'HALT' && (
              <div className="mt-2 text-xs text-text-muted">
                Please type exactly "HALT" to confirm
              </div>
            )}
          </div>
          <div className="flex gap-3 justify-end">
            <Button
              variant="secondary"
              onClick={() => {
                setShowKillSwitchModal(false)
                setKillSwitchConfirm('')
              }}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={handleEmergencyKillSwitch}
              loading={tripCircuitBreaker.isPending}
              disabled={killSwitchConfirm !== 'HALT'}
            >
              <Power className="w-4 h-4 mr-2" />
              Activate Kill Switch
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  )
}

// Read-only view for non-admin users
function ConfigReadOnly({ 
  config, 
  health 
}: { 
  config: ConfigResponse | null
  health: HealthResponse | null
}) {
  if (!config) return null

  return (
    <div className="space-y-6">
      <ConfigSection
        title="Circuit Breakers"
        defaultOpen={true}
        collapsible={false}
        badge={
          <Badge
            variant={health?.circuit_breaker?.trading_allowed ? 'success' : 'danger'}
          >
            {health?.circuit_breaker?.trading_allowed ? 'Active' : 'Tripped'}
          </Badge>
        }
      >
        <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
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
      </ConfigSection>

      <ConfigSection
        title="Strategy Allocation"
        defaultOpen={true}
        collapsible={false}
      >
        <div className="grid grid-cols-2 gap-4">
          <div className="bg-shield/10 border border-shield/30 rounded-lg p-4">
            <div className="text-shield font-semibold">🛡️ Shield</div>
            <div className="text-2xl font-mono-numbers mt-1">
              {config?.strategy_allocation?.shield_percent ?? 70}%
            </div>
          </div>
          <div className="bg-spear/10 border border-spear/30 rounded-lg p-4">
            <div className="text-spear font-semibold">⚔️ Spear</div>
            <div className="text-2xl font-mono-numbers mt-1">
              {config?.strategy_allocation?.spear_percent ?? 30}%
            </div>
          </div>
        </div>
      </ConfigSection>

      {/* Add more read-only sections for other configs */}
      {config.profit_management && (
        <ConfigSection
          title="Profit Management"
          defaultOpen={false}
        >
          <div className="grid grid-cols-2 md:grid-cols-3 gap-4 text-sm">
            <div>
              <div className="text-text-muted">Profit Targets</div>
              <div className="font-mono-numbers">
                {config.profit_management.targets.join('%, ')}%
              </div>
            </div>
            <div>
              <div className="text-text-muted">Tiered Exit</div>
              <div className="font-mono-numbers">{config.profit_management.tiered_exit_percent}%</div>
            </div>
            <div>
              <div className="text-text-muted">Hard Stop Loss</div>
              <div className="font-mono-numbers">{config.profit_management.hard_stop_loss}%</div>
            </div>
          </div>
        </ConfigSection>
      )}

      {config.monitoring && (
        <ConfigSection
          title="Monitoring"
          defaultOpen={false}
          badge={config.monitoring.enabled ? <Badge variant="success">Active</Badge> : <Badge variant="default">Inactive</Badge>}
        >
          <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
            <div>
              <div className="text-text-muted">Max Active Wallets</div>
              <div className="font-mono-numbers">{config.monitoring.max_active_wallets}</div>
            </div>
            <div>
              <div className="text-text-muted">Webhook Rate Limit</div>
              <div className="font-mono-numbers">{config.monitoring.webhook_processing_rate_limit} req/sec</div>
            </div>
            <div>
              <div className="text-text-muted">RPC Polling</div>
              <div className="font-mono-numbers">{config.monitoring.rpc_polling_enabled ? 'Enabled' : 'Disabled'}</div>
            </div>
            <div>
              <div className="text-text-muted">Poll Interval</div>
              <div className="font-mono-numbers">{config.monitoring.rpc_poll_interval_secs} sec</div>
            </div>
          </div>
        </ConfigSection>
      )}
    </div>
  )
}
