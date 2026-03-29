import { apiClient } from '@core/lib/api-client'

export interface UserSettings {
  language: string
}

export const userSettingsApi = {
  get: () =>
    apiClient.get<UserSettings>('/settings'),

  patch: (body: Partial<UserSettings>) =>
    apiClient.patch<UserSettings>('/settings', body),
}
