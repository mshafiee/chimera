import { describe, it, expect } from 'vitest'
import { toNum, safeToFixed } from '../format'

describe('toNum', () => {
  it('returns 0 for null, undefined and empty string', () => {
    expect(toNum(null)).toBe(0)
    expect(toNum(undefined)).toBe(0)
    expect(toNum('')).toBe(0)
  })

  it('returns 0 for non-number non-string values', () => {
    expect(toNum({})).toBe(0)
    expect(toNum([])).toBe(0)
    expect(toNum(true)).toBe(0)
  })

  it('returns number unchanged', () => {
    expect(toNum(42)).toBe(42)
    expect(toNum(-1.5)).toBe(-1.5)
  })

  it('parses numeric strings', () => {
    expect(toNum('12.34')).toBe(12.34)
    expect(toNum('0.020000000000000000')).toBe(0.02)
  })

  it('returns 0 for non-finite values', () => {
    expect(toNum('abc')).toBe(0)
    expect(toNum(Infinity)).toBe(0)
    expect(toNum(NaN)).toBe(0)
    expect(toNum('Infinity')).toBe(0)
  })
})

describe('safeToFixed', () => {
  it('returns padded zero for null/undefined/empty', () => {
    expect(safeToFixed(null)).toBe('0.00')
    expect(safeToFixed(undefined, 4)).toBe('0.0000')
    expect(safeToFixed('')).toBe('0.00')
  })

  it('returns padded zero for non number/string values', () => {
    expect(safeToFixed({}, 3)).toBe('0.000')
  })

  it('returns padded zero for non-finite values', () => {
    expect(safeToFixed('not-a-number')).toBe('0.00')
    expect(safeToFixed(NaN)).toBe('0.00')
  })

  it('formats numbers', () => {
    expect(safeToFixed(12.3456)).toBe('12.35')
    expect(safeToFixed(12.3456, 4)).toBe('12.3456')
  })

  it('formats string decimals', () => {
    expect(safeToFixed('0.020000000000000000', 4)).toBe('0.0200')
    expect(safeToFixed('123.456', 1)).toBe('123.5')
  })

  it('clamps decimals between 0 and 100', () => {
    expect(safeToFixed(1.5, 200)).toBe('1.' + '5'.padEnd(100, '0'))
    expect(safeToFixed(1.5, -3)).toBe('2')
    expect(safeToFixed(1.5, 1.9)).toBe('1.5')
  })

  it('falls back to 2 decimals when decimals is non-finite', () => {
    expect(safeToFixed(1.5, NaN)).toBe('1.50')
    expect(safeToFixed(1.5, Infinity)).toBe('1.50')
  })

  it('rounds decimals to integer via trunc', () => {
    expect(safeToFixed(1.5678, 2.9)).toBe('1.57')
  })
})
