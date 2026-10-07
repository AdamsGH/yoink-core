import { apiClient } from '@core/lib/api-client'

export interface MutationStatus {
  id: string
  state: 'prepared' | 'sending' | 'unknown' | 'confirmed' | 'applied' | 'rejected'
  folder_id?: number | null
  error_code?: string | null
  operation_key_matches?: boolean
  can_cancel?: boolean
  can_inspect_candidates?: boolean
}
export interface CreateCandidate { id: string; name: string; slug: string; description: string | null }
const route = (id: string) => `/inbox/gh_mutations/${encodeURIComponent(id)}`
export async function listMutations(): Promise<MutationStatus[]> {
  return (await apiClient.get<MutationStatus[]>('/inbox/gh_mutations')).data
}
export async function mutationStatus(id: string, operationKey?: string): Promise<MutationStatus> {
  return (await apiClient.get<MutationStatus>(route(id), {
    headers: operationKey ? { 'Idempotency-Key': operationKey } : undefined,
  })).data
}
export async function reconcileMutation(id: string): Promise<MutationStatus> {
  return (await apiClient.post<MutationStatus>(`${route(id)}/reconcile`)).data
}
export async function createCandidates(id: string): Promise<CreateCandidate[]> {
  return (await apiClient.get<CreateCandidate[]>(`${route(id)}/create-candidates`)).data
}
export async function adoptCreate(id: string, remoteId: string): Promise<MutationStatus> {
  return (await apiClient.post<MutationStatus>(`${route(id)}/adopt-create`, { remote_id: remoteId })).data
}
export async function cancelMutation(id: string): Promise<void> {
  await apiClient.post(`${route(id)}/cancel`)
}
export function pendingMutationId(error: unknown): string | undefined {
  const id = (error as { response?: { data?: { mutation_id?: unknown } } })?.response?.data?.mutation_id
  return typeof id === 'string' ? id : undefined
}
