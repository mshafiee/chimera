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
      // Clear auth on unauthorized
      useAuthStore.getState().logout()
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
