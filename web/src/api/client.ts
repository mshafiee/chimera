import axios, { AxiosError } from 'axios'
import { useAuthStore } from '../stores/authStore'

const API_BASE = '/api/v1'

export const apiClient = axios.create({
  baseURL: API_BASE,
  headers: {
    'Content-Type': 'application/json',
  },
})

// Add auth token to requests
apiClient.interceptors.request.use((config) => {
  const { user } = useAuthStore.getState()
  if (user?.token) {
    config.headers.Authorization = `Bearer ${user.token}`
  }
  return config
})

// Handle errors
apiClient.interceptors.response.use(
  (response) => response,
  (error: AxiosError<{ reason?: string; details?: string }>) => {
    if (error.response?.status === 401) {
      // Only logout on 401 if it's clearly an authentication failure
      // Don't logout on 401 for operation failures (let components handle those)
      const url = error.config?.url || ''
      const isAuthEndpoint = url.includes('/auth/')
      
      // Only auto-logout if it's an auth endpoint (login/authentication failed)
      // For other endpoints, it might be a permission issue or invalid token - let component handle it
      if (isAuthEndpoint) {
        useAuthStore.getState().logout()
      }
      // For non-auth endpoints, don't auto-logout - let the component show the error
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
