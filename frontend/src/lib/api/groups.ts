import { apiClient } from '@core/lib/api-client'
import type { Group, GroupCreateRequest, GroupUpdateRequest, ThreadPolicy } from '@core/types/api'

export type { ThreadPolicy }

export const groupsApi = {
  list: () =>
    apiClient.get<Group[]>('/groups'),

  create: (body: GroupCreateRequest) =>
    apiClient.post<Group>('/groups', body),

  update: (id: number, body: GroupUpdateRequest) =>
    apiClient.patch<Group>(`/groups/${id}`, body),

  listThreads: (groupId: number) =>
    apiClient.get<ThreadPolicy[]>(`/groups/${groupId}/threads`),

  addThread: (groupId: number, body: { thread_id: number; name: string | null; enabled: boolean }) =>
    apiClient.post<ThreadPolicy>(`/groups/${groupId}/threads`, body),

  updateThread: (groupId: number, policy: ThreadPolicy) =>
    apiClient.post<ThreadPolicy>(`/groups/${groupId}/threads`, {
      thread_id: policy.thread_id,
      name: policy.name,
      enabled: !policy.enabled,
    }),

  deleteThread: (groupId: number, policyId: number) =>
    apiClient.delete(`/groups/${groupId}/threads/${policyId}`),

  photoUrl: (groupId: number) =>
    `${apiClient.defaults.baseURL}/groups/${groupId}/photo`,
}
