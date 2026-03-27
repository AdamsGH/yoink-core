import axios from 'axios'

const BASE_URL = import.meta.env.VITE_API_URL ?? (import.meta.env.DEV ? 'http://localhost:8003/api/v1' : '/api/v1')

export const apiClient = axios.create({ baseURL: BASE_URL })

apiClient.interceptors.request.use((config) => {
  const token = localStorage.getItem('access_token')
  if (token) {
    config.headers.Authorization = `Bearer ${token}`
  }
  return config
})

apiClient.interceptors.response.use(
  (r) => r,
  (err: unknown) => {
    if (axios.isAxiosError(err) && err.response?.status === 401) {
      const url = err.config?.url ?? ''
      // Only reload on auth endpoints - a 401 on a role-protected endpoint
      // just means insufficient permissions, not an expired session.
      const isAuthEndpoint = url.includes('/auth/') || url.includes('/users/me')
      if (isAuthEndpoint) {
        localStorage.removeItem('access_token')
        window.location.reload()
      }
    }
    return Promise.reject(err)
  }
)
