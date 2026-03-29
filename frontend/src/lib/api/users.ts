import { apiClient } from '@core/lib/api-client'
import type { PaginatedResponse, User, UserUpdateRequest, EffectiveFeatureAccess } from '@core/types/api'

export type UserSortField = 'created_at' | 'updated_at' | 'name' | 'role' | 'dl_count' | 'dl_last_at'
export type SortDirection = 'asc' | 'desc'

export interface UsersQuery {
  page?: number
  limit?: number
  search?: string
  role?: string
  status?: string
  offset?: number
  sort?: UserSortField
  direction?: SortDirection
}

export interface UserStats {
  total: number
  this_week: number
  today: number
  dl_last_at: string | null
  top_domains: Array<{ domain: string; count: number }>
  member_since: string
  music_total: number
  music_last_at: string | null
  ai_total: number
  ai_last_at: string | null
  plugins: string[]
}

export const usersApi = {
  list: (query: UsersQuery = {}) =>
    apiClient.get<PaginatedResponse<User>>('/users', { params: query }),

  get: (id: number) =>
    apiClient.get<User>(`/users/${id}`),

  update: (id: number, body: UserUpdateRequest) =>
    apiClient.patch<User>(`/users/${id}`, body),

  getStats: (id: number, statsEndpoint?: string) =>
    apiClient.get<UserStats>(statsEndpoint ?? `/users/${id}/stats`),

  getFeatureAccess: (id: number) =>
    apiClient.get<EffectiveFeatureAccess[]>(`/users/${id}/feature-access`),

  grantPermission: (userId: number, plugin: string, feature: string) =>
    apiClient.post(`/users/${userId}/permissions`, { plugin, feature }),

  revokePermission: (userId: number, plugin: string, feature: string) =>
    apiClient.delete(`/users/${userId}/permissions/${plugin}/${feature}`),

  photoUrl: (userId: number) =>
    `${apiClient.defaults.baseURL}/users/${userId}/photo`,
}
