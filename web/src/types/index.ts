// API Response Types

export interface Position {
  id: number
  trade_uuid: string
  wallet_address: string
  token_address: string
  token_symbol: string | null
  strategy: 'SHIELD' | 'SPEAR'
  entry_amount_sol: number
  entry_price: number
  entry_tx_signature: string
  current_price: number | null
  unrealized_pnl_sol: number | null
  unrealized_pnl_percent: number | null
  state: 'ACTIVE' | 'EXITING' | 'CLOSED'
  exit_price: number | null
  exit_tx_signature: string | null
  realized_pnl_sol: number | null
  realized_pnl_usd: number | null
  opened_at: string
  last_updated: string
  closed_at: string | null
}

export interface Wallet {
  id: number
  address: string
  status: 'ACTIVE' | 'CANDIDATE' | 'REJECTED'
  wqs_score: number | null
  roi_7d: number | null
  roi_30d: number | null
  trade_count_30d: number | null
  win_rate: number | null
  max_drawdown_30d: number | null
  avg_trade_size_sol: number | null
  last_trade_at: string | null
  promoted_at: string | null
  ttl_expires_at: string | null
  notes: string | null
  created_at: string
  updated_at: string
}

export interface Trade {
  id: number
  trade_uuid: string
  wallet_address: string
  token_address: string
  token_symbol: string | null
  strategy: 'SHIELD' | 'SPEAR' | 'EXIT'
  side: 'BUY' | 'SELL'
  amount_sol: number
  price_at_signal: number | null
  tx_signature: string | null
  status: 'PENDING' | 'QUEUED' | 'EXECUTING' | 'ACTIVE' | 'EXITING' | 'CLOSED' | 'FAILED' | 'RETRY' | 'DEAD_LETTER'
  retry_count: number
  error_message: string | null
  pnl_sol: number | null
  pnl_usd: number | null
  created_at: string
  updated_at: string
}

export interface HealthResponse {
  status: 'healthy' | 'degraded' | 'unhealthy'
  uptime_seconds: number
  queue_depth: number
  rpc_latency_ms: number
  last_trade_at: string | null
  database: {
    status: 'healthy' | 'degraded' | 'unhealthy'
    message: string | null
  }
  rpc: {
    status: 'healthy' | 'degraded' | 'unhealthy'
    message: string | null
  }
  circuit_breaker: {
    state: string
    trading_allowed: boolean
    trip_reason: string | null
    cooldown_remaining_secs: number | null
  }
  price_cache: {
    total_entries: number
    tracked_tokens: number
  }
}

export interface ConfigResponse {
  circuit_breakers: {
    max_loss_24h: number
    max_consecutive_losses: number
    max_drawdown_percent: number
    cool_down_minutes: number
  }
  strategy_allocation: {
    shield_percent: number
    spear_percent: number
  }
  strategy: {
    max_position_sol: number
    min_position_sol: number
  }
  jito_tip_strategy: {
    tip_floor: number
    tip_ceiling: number
    tip_percentile: number
    tip_percent_max: number
  }
  jito_enabled: boolean
  rpc_status: {
    primary: string
    active: string
    fallback_triggered: boolean
  }
  monitoring?: MonitoringConfig
  profit_management: ProfitManagementConfig
  position_sizing: PositionSizingConfig
  mev_protection: MevProtectionConfig
  token_safety: TokenSafetyConfig
  notifications: NotificationsConfig
  queue: QueueConfig
}

export interface MonitoringConfig {
  enabled: boolean
  webhook_registration_batch_size: number
  webhook_registration_delay_ms: number
  webhook_processing_rate_limit: number
  rpc_polling_enabled: boolean
  rpc_poll_interval_secs: number
  rpc_poll_batch_size: number
  rpc_poll_rate_limit: number
  max_active_wallets: number
}

export interface ProfitManagementConfig {
  targets: number[]
  tiered_exit_percent: number
  trailing_stop_activation: number
  trailing_stop_distance: number
  hard_stop_loss: number
  time_exit_hours: number
}

export interface PositionSizingConfig {
  base_size_sol: number
  max_size_sol: number
  min_size_sol: number
  consensus_multiplier: number
  max_concurrent_positions: number
}

export interface MevProtectionConfig {
  always_use_jito: boolean
  exit_tip_sol: number
  consensus_tip_sol: number
  standard_tip_sol: number
}

export interface TokenSafetyConfig {
  min_liquidity_shield_usd: number
  min_liquidity_spear_usd: number
  honeypot_detection_enabled: boolean
  cache_capacity: number
  cache_ttl_seconds: number
  freeze_authority_whitelist: string[]
  mint_authority_whitelist: string[]
}

export interface NotificationsConfig {
  telegram: {
    enabled: boolean
    rate_limit_seconds: number
  }
  rules: {
    circuit_breaker_triggered: boolean
    wallet_drained: boolean
    position_exited: boolean
    wallet_promoted: boolean
    daily_summary: boolean
    rpc_fallback: boolean
  }
  daily_summary: {
    enabled: boolean
    hour_utc: number
    minute: number
  }
}

export interface QueueConfig {
  capacity: number
  load_shed_threshold_percent: number
}

export interface Incident {
  id: number
  trade_uuid: string | null
  payload: string
  reason: string
  error_details: string | null
  source_ip: string | null
  retry_count: number
  can_retry: boolean
  received_at: string
  processed_at: string | null
}

export interface ConfigAudit {
  id: number
  key: string
  old_value: string | null
  new_value: string
  changed_by: string
  change_reason: string | null
  changed_at: string
}

export type Role = 'readonly' | 'operator' | 'admin'

export interface AuthUser {
  identifier: string
  role: Role
  token: string
}
