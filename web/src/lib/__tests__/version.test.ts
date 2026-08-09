import { describe, it, expect, vi, beforeEach } from 'vitest'

async function loadVersion(): Promise<string> {
  vi.resetModules()
  const mod = await import('../version')
  return mod.APP_VERSION
}

describe('APP_VERSION', () => {
  beforeEach(() => {
    vi.unstubAllGlobals()
  })

  it('prepends v to the injected version', async () => {
    vi.stubGlobal('__APP_VERSION__', '1.2.3')
    expect(await loadVersion()).toBe('v1.2.3')
  })

  it('falls back to 0.0.0 when version is not injected', async () => {
    vi.stubGlobal('__APP_VERSION__', undefined)
    expect(await loadVersion()).toBe('v0.0.0')
  })
})
