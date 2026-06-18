import axios, { AxiosError, InternalAxiosRequestConfig } from 'axios'
import { useAuthStore } from '../stores/authStore'

const API_BASE = '/api/v1'

interface AxiosRequestConfigWithRetry extends InternalAxiosRequestConfig {
  _retry?: boolean
}

export const apiClient = axios.create({
  baseURL: API_BASE,
  headers: {
    'Content-Type': 'application/json',
  },
})

let isRefreshing = false
let failedQueue: Array<{
  resolve: (value?: unknown) => void
  reject: (reason?: unknown) => void
}> = []

const processQueue = (error: unknown, token: string | null = null) => {
  failedQueue.forEach((prom) => {
    if (error) {
      prom.reject(error)
    } else {
      prom.resolve(token)
    }
  })
  failedQueue = []
}

// Add auth token to requests
apiClient.interceptors.request.use((config) => {
  const authState = useAuthStore.getState()

  // Check if session has expired
  if (authState.isSessionExpired()) {
    authState.logout()
    return Promise.reject(new Error('Session expired due to inactivity'))
  }

  // Check if token is expired and we're not already trying to refresh
  if (authState.isTokenExpired() && !config.url?.includes('/auth/refresh')) {
    // If we have a refresh token, we'll handle it in the response interceptor
    // For now, just add the current token
  }

  if (authState.user?.token) {
    config.headers.Authorization = `Bearer ${authState.user.token}`
  }
  return config
})

// Handle errors and token refresh
apiClient.interceptors.response.use(
  (response) => response,
  async (error: AxiosError<{ reason?: string; details?: string }>) => {
    const originalRequest = error.config as AxiosRequestConfigWithRetry
    const authState = useAuthStore.getState()

    // Handle 401 errors
    if (error.response?.status === 401 && !originalRequest._retry) {
      const url = originalRequest.url || ''
      const isAuthEndpoint = url.includes('/auth/')
      const isRefreshEndpoint = url.includes('/auth/refresh')

      // If it's the refresh endpoint that failed, logout and reject
      if (isRefreshEndpoint) {
        authState.logout()
        processQueue(error)
        return Promise.reject(error)
      }

      // If it's the login endpoint that failed, just reject (don't refresh)
      if (isAuthEndpoint) {
        authState.logout()
        return Promise.reject(error)
      }

      // For other endpoints, try to refresh the token
      if (authState.refreshToken && !isRefreshing) {
        isRefreshing = true
        originalRequest._retry = true

        try {
          // Attempt to refresh the token
          const response = await axios.post(`${API_BASE}/auth/refresh`, {
            refresh_token: authState.refreshToken,
          })

          const { access_token, refresh_token: newRefreshToken, expires_in } = response.data

          // Update the auth store with new tokens
          authState.updateTokens(access_token, newRefreshToken, expires_in)

          // Update the header for the original request
          originalRequest.headers.Authorization = `Bearer ${access_token}`

          processQueue(null, access_token)
          return apiClient(originalRequest)
        } catch (refreshError) {
          // Refresh failed, logout and reject all queued requests
          authState.logout()
          processQueue(refreshError)
          return Promise.reject(refreshError)
        } finally {
          isRefreshing = false
        }
      } else if (authState.refreshToken && isRefreshing) {
        // If we're already refreshing, add this request to the queue
        return new Promise((resolve, reject) => {
          failedQueue.push({ resolve, reject })
        })
          .then((token) => {
            originalRequest.headers.Authorization = `Bearer ${token}`
            return apiClient(originalRequest)
          })
          .catch((err) => {
            return Promise.reject(err)
          })
      } else {
        // No refresh token available, logout
        authState.logout()
        return Promise.reject(error)
      }
    }

    return Promise.reject(error)
  }
)

export interface ApiError {
  status: string
  reason: string
  details?: string
}

export function getApiError(error: unknown): string {
  if (axios.isAxiosError(error)) {
    const data = error.response?.data as ApiError | undefined
    return data?.details || data?.reason || error.message
  }
  if (error instanceof Error) {
    return error.message
  }
  return 'An unknown error occurred'
}
