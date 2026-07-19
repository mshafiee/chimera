// Formatting helpers for API decimal fields.
//
// The operator API serializes Postgres NUMERIC / rust_decimal::Decimal values
// as JSON strings (e.g. "0.020000000000000000"). Calling .toFixed() directly
// on these values throws "x.toFixed is not a function". Use these helpers to
// safely coerce before formatting or comparing.

/**
 * Safely convert any value (string | number | null | undefined) to a number.
 * Returns 0 for null/undefined/empty/NaN inputs.
 */
export function toNum(value: unknown): number {
  if (value === null || value === undefined || value === '') return 0
  const n = typeof value === 'number' ? value : Number(value)
  return isNaN(n) ? 0 : n
}

/**
 * Safely format a value to a fixed number of decimals.
 * Handles string-encoded decimals from the API without throwing.
 * Returns '0.00...' style placeholder for null/undefined/NaN.
 */
export function safeToFixed(value: unknown, decimals: number = 2): string {
  if (value === null || value === undefined || value === '') {
    return '0.' + '0'.repeat(decimals)
  }
  const num = typeof value === 'number' ? value : Number(value)
  if (isNaN(num)) {
    return '0.' + '0'.repeat(decimals)
  }
  return num.toFixed(decimals)
}
