import { describe, it, expect, vi, beforeEach } from 'vitest'

const queryCacheCtor = vi.hoisted(() => vi.fn())
const activityTrackerMock = vi.hoisted(() => vi.fn())

vi.mock('@tanstack/react-query', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@tanstack/react-query')>()
  return {
    ...actual,
    QueryCache: vi.fn(function (this: unknown, options: unknown) {
      queryCacheCtor(options)
      return new actual.QueryCache(options as never)
    }),
  }
})

vi.mock('../App', () => ({
  default: () => <div>Mocked App</div>,
}))

vi.mock('../components/wallet', () => ({
  WalletProvider: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

vi.mock('../hooks/useActivityTracker', () => ({
  useActivityTracker: activityTrackerMock,
}))

let cacheOptions: {
  onError: (error: unknown, query: { meta?: { onError?: (e: unknown) => void } }) => void
} | null = null

beforeEach(() => {
  vi.clearAllMocks()
  document.body.innerHTML = '<div id="root"></div>'
})

describe('main', () => {
  it('mounts the app tree into the root element', async () => {
    await import('../main')
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain('Mocked App')
    })
    expect(activityTrackerMock).toHaveBeenCalledWith(true)
    cacheOptions = queryCacheCtor.mock.calls[0][0] as typeof cacheOptions
    expect(cacheOptions).toBeDefined()
  })

  it('forwards query meta errors through the query cache', () => {
    expect(cacheOptions).not.toBeNull()
    const metaHandler = vi.fn()
    cacheOptions!.onError(new Error('boom'), { meta: { onError: metaHandler } })
    expect(metaHandler).toHaveBeenCalledWith(new Error('boom'))

    // non-function meta handlers are ignored
    cacheOptions!.onError(new Error('boom'), { meta: {} })
    cacheOptions!.onError(new Error('boom'), { meta: undefined })
  })
})
