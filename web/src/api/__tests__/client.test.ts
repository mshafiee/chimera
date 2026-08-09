import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import axios from 'axios'
import { useAuthStore } from '../../stores/authStore'
import { getApiError } from '../client'

const { mockAxiosInstance } = vi.hoisted(() => {
  const mockAxiosInstance = vi.fn() as unknown as {
    get: ReturnType<typeof vi.fn>
    post: ReturnType<typeof vi.fn>
    put: ReturnType<typeof vi.fn>
    interceptors: {
      request: { use: ReturnType<typeof vi.fn> }
      response: { use: ReturnType<typeof vi.fn> }
    }
  }
  mockAxiosInstance.get = vi.fn()
  mockAxiosInstance.post = vi.fn()
  mockAxiosInstance.put = vi.fn()
  mockAxiosInstance.interceptors = { request: { use: vi.fn() }, response: { use: vi.fn() } }
  return { mockAxiosInstance }
})

vi.mock('axios', () => ({
  default: {
    create: vi.fn(() => mockAxiosInstance),
    isAxiosError: (e: unknown) =>
      !!(e && typeof e === 'object' && (e as { isAxiosError?: boolean }).isAxiosError),
    post: vi.fn(),
  },
}))

const mockedAxios = vi.mocked(axios, true)

// Interceptor callbacks are registered once at module import time — capture
// them immediately (vi.clearAllMocks would erase the call history later).
const requestInterceptor = mockAxiosInstance.interceptors.request.use.mock.calls[0][0]
const responseErrorInterceptor = mockAxiosInstance.interceptors.response.use.mock.calls[0][1]

function makeError(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    isAxiosError: true,
    message: 'Request failed with status code 500',
    config: { url: '/positions', headers: {} },
    response: { status: 401, data: {} },
    ...overrides,
  }
}

function resetAuthStore() {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
}

describe('apiClient', () => {
  beforeEach(() => {
    resetAuthStore()
    mockedAxios.post.mockClear()
    mockAxiosInstance.mockClear()
    mockAxiosInstance.get.mockClear()
    mockAxiosInstance.put.mockClear()
    mockAxiosInstance.post.mockClear()
    // default: the retried request resolves with a dummy payload
    mockAxiosInstance.mockResolvedValue({ data: { ok: true } })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('creates the axios client with base URL and JSON content type', () => {
    expect(mockedAxios.create).toHaveBeenCalledWith({
      baseURL: '/api/v1',
      headers: { 'Content-Type': 'application/json' },
    })
  })

  it('registers request and response interceptors', () => {
    expect(mockAxiosInstance.interceptors.request.use).toHaveBeenCalledTimes(1)
    expect(mockAxiosInstance.interceptors.response.use).toHaveBeenCalledTimes(1)
  })

  describe('request interceptor', () => {
    it('rejects when the session has expired due to inactivity', () => {
      useAuthStore.setState({
        isAuthenticated: true,
        lastActivity: Date.now() - 31 * 60 * 1000,
      })
      const result = requestInterceptor({ url: '/positions' })
      expect(useAuthStore.getState().isAuthenticated).toBe(false)
      return expect(result).rejects.toThrow('Session expired due to inactivity')
    })

    it('rejects when the token is expired and no refresh token exists', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: null,
        lastActivity: Date.now(),
      })
      const result = requestInterceptor({ url: '/positions' })
      expect(useAuthStore.getState().user).toBeNull()
      return expect(result).rejects.toThrow('Session expired')
    })

    it('skips expiry handling for the auth refresh endpoint', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: 'refresh-token',
        lastActivity: Date.now(),
      })
      const config = { url: '/auth/refresh', headers: {} }
      const result = requestInterceptor(config) as Promise<unknown> | Record<string, unknown>
      expect(result).toBe(config)
      expect(config.headers.Authorization).toBe('Bearer expired.jwt')
    })

    it('passes an expired token through when a refresh token exists and the URL is not the refresh endpoint', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: 'refresh-token',
        lastActivity: Date.now(),
      })
      const config = { url: '/positions', headers: {} }
      const result = requestInterceptor(config) as Promise<unknown> | Record<string, unknown>
      expect(result).toBe(config)
      expect(config.headers.Authorization).toBe('Bearer expired.jwt')
      expect(useAuthStore.getState().user).not.toBeNull()
    })

    it('attaches the Authorization header when a token is present', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'fresh.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() + 3600000,
        lastActivity: Date.now(),
      })
      const config = { url: '/positions', headers: {} }
      const result = requestInterceptor(config) as Promise<unknown> | Record<string, unknown>
      expect(result).toBe(config)
      expect(config.headers.Authorization).toBe('Bearer fresh.jwt')
    })

    it('passes the config through when there is no token', () => {
      const config = { url: '/positions', headers: {} }
      const result = requestInterceptor(config) as Promise<unknown> | Record<string, unknown>
      expect(result).toBe(config)
      expect(config.headers.Authorization).toBeUndefined()
    })
  })

  describe('response error interceptor', () => {
    it('rejects non-401 errors unchanged', () => {
      const error = makeError({ response: { status: 500, data: { reason: 'boom' } } })
      return expect(responseErrorInterceptor(error)).rejects.toBe(error)
    })

    it('logs out and rejects when the refresh endpoint itself fails with 401', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 't' },
        isAuthenticated: true,
        refreshToken: 'refresh',
      })
      const error = makeError({ config: { url: '/auth/refresh', headers: {} } })
      return responseErrorInterceptor(error).catch(() => {
        expect(useAuthStore.getState().user).toBeNull()
      })
    })

    it('rejects auth endpoints (e.g. wallet login) without wiping the session', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 't' },
        isAuthenticated: true,
        refreshToken: 'refresh',
      })
      const error = makeError({ config: { url: '/auth/wallet', headers: {} } })
      return responseErrorInterceptor(error).catch(() => {
        expect(useAuthStore.getState().user).not.toBeNull()
      })
    })

    it('refreshes the token, drains the queue and retries the request', async () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: 'refresh-token',
      })
      mockedAxios.post.mockResolvedValue({
        data: { access_token: 'new.jwt', refresh_token: 'new-refresh', expires_in: 3600 },
      })

      const error = makeError({ config: { url: '/positions', headers: {} } })
      await responseErrorInterceptor(error)

      expect(mockedAxios.post).toHaveBeenCalledWith('/api/v1/auth/refresh', {
        token: 'refresh-token',
      })
      expect(useAuthStore.getState().user?.token).toBe('new.jwt')
      expect(useAuthStore.getState().refreshToken).toBe('new-refresh')
      expect(mockAxiosInstance).toHaveBeenCalled()
    })

    it('logs out and rejects queued requests when refresh fails', async () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: 'refresh-token',
      })
      mockedAxios.post.mockRejectedValue(new Error('refresh failed'))

      const error = makeError({ config: { url: '/positions', headers: {} } })
      await expect(responseErrorInterceptor(error)).rejects.toThrow('refresh failed')
      expect(useAuthStore.getState().user).toBeNull()
    })

    it('queues concurrent 401s while a refresh is in flight and drains them on success', async () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: 'refresh-token',
      })

      let resolveRefresh: (value: unknown) => void
      mockedAxios.post.mockReturnValue(
        new Promise((resolve) => {
          resolveRefresh = resolve
        })
      )

      const first = responseErrorInterceptor(makeError({ config: { url: '/positions', headers: {} } }))
      const queued = responseErrorInterceptor(makeError({ config: { url: '/trades', headers: {} } }))

      resolveRefresh!({
        data: { access_token: 'new.jwt', refresh_token: 'r', expires_in: 3600 },
      })

      await first
      await queued
      expect(mockAxiosInstance).toHaveBeenCalledTimes(2)
    })

    it('rejects queued requests when the shared refresh fails', async () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        tokenExpiresAt: Date.now() - 1000,
        refreshToken: 'refresh-token',
      })

      let rejectRefresh: (reason: unknown) => void
      mockedAxios.post.mockReturnValue(
        new Promise((_resolve, reject) => {
          rejectRefresh = reject
        })
      )

      const first = responseErrorInterceptor(makeError({ config: { url: '/positions', headers: {} } }))
      const queued = responseErrorInterceptor(makeError({ config: { url: '/trades', headers: {} } }))

      rejectRefresh!(new Error('refresh failed'))

      await expect(first).rejects.toThrow('refresh failed')
      await expect(queued).rejects.toThrow('refresh failed')
    })

    it('logs out and rejects when 401 occurs without a refresh token', () => {
      useAuthStore.setState({
        user: { identifier: 'u', role: 'admin', token: 'expired.jwt' },
        isAuthenticated: true,
        refreshToken: null,
      })
      const error = makeError({ config: { url: '/positions', headers: {} } })
      return responseErrorInterceptor(error).catch(() => {
        expect(useAuthStore.getState().user).toBeNull()
      })
    })
  })
})

describe('getApiError', () => {
  it('returns details from the error payload', () => {
    const error = makeError({
      response: { status: 400, data: { details: 'detailed message', reason: 'reason' } },
    })
    expect(getApiError(error)).toBe('detailed message')
  })

  it('returns reason when details are missing', () => {
    const error = makeError({ response: { status: 400, data: { reason: 'reason message' } } })
    expect(getApiError(error)).toBe('reason message')
  })

  it('falls back to the error message', () => {
    const error = makeError({
      message: 'plain axios message',
      response: { status: 500, data: {} },
    })
    expect(getApiError(error)).toBe('plain axios message')
  })

  it('returns the message for plain Error instances', () => {
    expect(getApiError(new Error('plain error'))).toBe('plain error')
  })

  it('returns a generic message for unknown values', () => {
    expect(getApiError('garbage')).toBe('An unknown error occurred')
    expect(getApiError(null)).toBe('An unknown error occurred')
  })
})
