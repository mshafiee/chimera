// Formatting helpers for API decimal fields.
//
// The operator API serializes Postgres NUMERIC / rust_decimal::Decimal values
// as JSON strings (e.g. "0.020000000000000000"). Calling .toFixed() directly
// on these values throws "x.toFixed is not a function". Use these helpers to
// safely coerce before formatting.
//
// NOTE: Values are coerced through JS numbers (~15-17 significant digits), so
// results are display-accurate but not exact for high-precision decimals. If
// comparison accuracy matters, compare the original strings instead.

/**
 * Safely convert any value (string | number | null | undefined) to a number.
 * Returns 0 for null/undefined/empty/non-finite inputs.
 */
export function toNum(value: unknown): number {
  if (value === null || value === undefined || value === '') return 0
  if (typeof value !== 'number' && typeof value !== 'string') return 0
  const n = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(n) ? n : 0
}

/**
 * Safely format a value to a fixed number of decimals.
 * Handles string-encoded decimals from the API without throwing.
 * Returns '0.00...' style placeholder for null/undefined/non-finite values.
 */
export function safeToFixed(value: unknown, decimals: number = 2): string {
  const d = Number.isFinite(decimals) ? Math.min(100, Math.max(0, Math.trunc(decimals))) : 2
  if (value === null || value === undefined || value === '') {
    return '0.' + '0'.repeat(d)
  }
  if (typeof value !== 'number' && typeof value !== 'string') {
    return '0.' + '0'.repeat(d)
  }
  const num = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(num)) {
    return '0.' + '0'.repeat(d)
  }
  return num.toFixed(d)
}
