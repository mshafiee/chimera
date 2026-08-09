import { describe, it, expect } from 'vitest'
import * as hooks from '../index'

describe('hooks barrel', () => {
  it('re-exports useWebSocket', () => {
    expect(typeof hooks.useWebSocket).toBe('function')
  })
})
