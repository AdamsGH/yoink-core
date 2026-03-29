import { apiClient } from '@core/lib/api-client'

export type BotSettings = Record<string, string | null | unknown>

import type { AvailableFeature, TagMapEntry } from '@core/types/api'
export type { AvailableFeature, TagMapEntry }

export const botSettingsApi = {
  get: () =>
    apiClient.get<Record<string, string | null>>('/bot-settings'),

  patch: (key: string, value: unknown) =>
    apiClient.patch('/bot-settings', { [key]: value }),

  patchMany: (body: Partial<BotSettings>) =>
    apiClient.patch('/bot-settings', body),

  getAvailableFeatures: () =>
    apiClient.get<AvailableFeature[]>('/bot-settings/available-features'),

  getTagMap: () =>
    apiClient.get<TagMapEntry[]>('/bot-settings/tag-map'),

  putTagMap: (entries: TagMapEntry[]) =>
    apiClient.put<TagMapEntry[]>('/bot-settings/tag-map', entries),
}
