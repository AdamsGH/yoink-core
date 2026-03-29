import { apiClient } from '@core/lib/api-client'
import type { Feature, Permission } from '@core/types/api'

export type { Feature, Permission }

export const permissionsApi = {
  listAll: () =>
    apiClient.get<Permission[]>('/permissions/all'),

  listFeatures: () =>
    apiClient.get<Feature[]>('/features'),

  grant: (userId: number, plugin: string, feature: string) =>
    apiClient.post<Permission>(`/users/${userId}/permissions`, { plugin, feature }),

  revoke: (userId: number, plugin: string, feature: string) =>
    apiClient.delete(`/users/${userId}/permissions/${plugin}/${feature}`),
}
