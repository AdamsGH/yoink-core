import { apiClient } from '@core/lib/api-client'

export interface ThreadsStatus {
  available: boolean
}

export interface ThreadsScanResult {
  total_count: number
  upserted: number
}

export const threadsApi = {
  getStatus: () =>
    apiClient.get<ThreadsStatus>('/threads/status'),

  scan: (groupId: number) =>
    apiClient.post<ThreadsScanResult>(`/threads/scan/${groupId}`),
}
