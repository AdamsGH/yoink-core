import { apiClient } from '@core/lib/api-client'

import type {
  InboxCategory,
  InboxGhStarListResponse,
  InboxItem,
  InboxItemListResponse,
} from '@inbox/types'

export interface InboxItemFilters {
  cursor?: string
  limit?: number
  status?: string
  kind?: string
  category_id?: number
  search?: string
}

export async function listItems(filters: InboxItemFilters = {}): Promise<InboxItemListResponse> {
  const { data } = await apiClient.get<InboxItemListResponse>('/inbox/items', {
    params: filters,
  })
  return data
}

export async function getItem(id: number): Promise<InboxItem> {
  const { data } = await apiClient.get<InboxItem>(`/inbox/items/${id}`)
  return data
}

export async function createItem(url: string, source = 'web'): Promise<InboxItem> {
  const { data } = await apiClient.post<InboxItem>('/inbox/items', { url, source })
  return data
}

export async function deleteItem(id: number, hard = false): Promise<void> {
  await apiClient.delete(`/inbox/items/${id}`, { params: { hard } })
}

export async function reclassifyItem(id: number): Promise<{ status: string }> {
  const { data } = await apiClient.post<{ status: string }>(
    `/inbox/items/${id}/reclassify`,
  )
  return data
}

export async function listCategories(): Promise<InboxCategory[]> {
  const { data } = await apiClient.get<InboxCategory[]>('/inbox/categories')
  return data
}

export interface GhStarFilters {
  cursor?: string
  limit?: number
  language?: string
  search?: string
}

export async function listGhStars(
  filters: GhStarFilters = {},
): Promise<InboxGhStarListResponse> {
  const { data } = await apiClient.get<InboxGhStarListResponse>(
    '/inbox/gh_stars',
    { params: filters },
  )
  return data
}

export async function triggerGhSync(): Promise<{ status: string }> {
  const { data } = await apiClient.post<{ status: string }>('/inbox/gh_stars/sync')
  return data
}
