import { apiClient } from '@core/lib/api-client'

import type {
  InboxCategory,
  InboxCategoryCreate,
  InboxGhFolder,
  InboxGhFolderCreate,
  InboxGhFolderPatch,
  InboxGhStar,
  InboxGhStarListResponse,
  InboxItem,
  InboxItemListResponse,
  InboxTeam,
  InboxTeamCreate,
  InboxTeamMember,
} from '@inbox/types'

export interface InboxItemFilters {
  cursor?: string
  limit?: number
  status?: string
  kind?: string
  category_id?: number
  uncategorized?: boolean
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

export async function setItemCategories(itemId: number, categoryIds: number[]): Promise<void> {
  await apiClient.put(`/inbox/items/${itemId}/categories`, categoryIds)
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

export type GhStarSort = 'starred_at' | 'stargazers_count' | 'name'

export interface GhStarFilters {
  cursor?: string
  offset?: number
  limit?: number
  language?: string
  search?: string
  folder_id?: number
  sort?: GhStarSort
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

// ---------------------------------------------------------------------------
// Categories CRUD
// ---------------------------------------------------------------------------

export async function createCategory(body: InboxCategoryCreate): Promise<InboxCategory> {
  const { data } = await apiClient.post<InboxCategory>('/inbox/categories', body)
  return data
}

export async function updateCategory(id: number, body: InboxCategoryCreate): Promise<InboxCategory> {
  const { data } = await apiClient.put<InboxCategory>(`/inbox/categories/${id}`, body)
  return data
}

export async function deleteCategory(id: number): Promise<void> {
  await apiClient.delete(`/inbox/categories/${id}`)
}

// ---------------------------------------------------------------------------
// GH Folders
// ---------------------------------------------------------------------------

export async function listFolders(): Promise<InboxGhFolder[]> {
  const { data } = await apiClient.get<InboxGhFolder[]>('/inbox/folders')
  return data
}

export async function createFolder(body: InboxGhFolderCreate): Promise<InboxGhFolder> {
  const { data } = await apiClient.post<InboxGhFolder>('/inbox/folders', body)
  return data
}

export async function updateFolder(id: number, body: InboxGhFolderCreate): Promise<InboxGhFolder> {
  const { data } = await apiClient.put<InboxGhFolder>(`/inbox/folders/${id}`, body)
  return data
}

export async function patchFolder(id: number, body: InboxGhFolderPatch): Promise<InboxGhFolder> {
  const { data } = await apiClient.patch<InboxGhFolder>(`/inbox/folders/${id}`, body)
  return data
}

export async function deleteFolder(id: number): Promise<void> {
  await apiClient.delete(`/inbox/folders/${id}`)
}

export async function addStarToFolder(folderId: number, starId: number): Promise<void> {
  await apiClient.post(`/inbox/folders/${folderId}/stars`, null, { params: { star_id: starId } })
}

export async function removeStarFromFolder(folderId: number, starId: number): Promise<void> {
  await apiClient.delete(`/inbox/folders/${folderId}/stars/${starId}`)
}

export async function listFolderStars(folderId: number): Promise<InboxGhStar[]> {
  // stars in a folder come via the regular gh_stars endpoint filtered by folder_id
  const { data } = await apiClient.get<InboxGhStarListResponse>('/inbox/gh_stars', {
    params: { folder_id: folderId, limit: 200 },
  })
  return data.items
}

// ---------------------------------------------------------------------------
// Teams
// ---------------------------------------------------------------------------

export async function listTeams(): Promise<InboxTeam[]> {
  const { data } = await apiClient.get<InboxTeam[]>('/inbox/teams')
  return data
}

export async function createTeam(body: InboxTeamCreate): Promise<InboxTeam> {
  const { data } = await apiClient.post<InboxTeam>('/inbox/teams', body)
  return data
}

export async function updateTeam(id: number, body: InboxTeamCreate): Promise<InboxTeam> {
  const { data } = await apiClient.put<InboxTeam>(`/inbox/teams/${id}`, body)
  return data
}

export async function deleteTeam(id: number): Promise<void> {
  await apiClient.delete(`/inbox/teams/${id}`)
}

export async function addTeamMember(teamId: number, userId: number, role: string): Promise<InboxTeamMember> {
  const { data } = await apiClient.post<InboxTeamMember>(`/inbox/teams/${teamId}/members`, { user_id: userId, role })
  return data
}

export async function patchTeamMember(teamId: number, userId: number, role: string): Promise<InboxTeamMember> {
  const { data } = await apiClient.patch<InboxTeamMember>(`/inbox/teams/${teamId}/members/${userId}`, { user_id: userId, role })
  return data
}

export async function removeTeamMember(teamId: number, userId: number): Promise<void> {
  await apiClient.delete(`/inbox/teams/${teamId}/members/${userId}`)
}

// ---------------------------------------------------------------------------
// Star / unstar (requires gh_write feature + public_repo token)
// ---------------------------------------------------------------------------

export async function starRepo(starId: number): Promise<void> {
  await apiClient.put(`/inbox/gh_stars/${starId}/star`)
}

export async function unstarRepo(starId: number): Promise<void> {
  await apiClient.delete(`/inbox/gh_stars/${starId}/star`)
}

export async function listStarLanguages(): Promise<string[]> {
  const { data } = await apiClient.get<string[]>('/inbox/gh_stars/languages')
  return data
}
