import { apiClient } from '@core/lib/api-client'

import type { InboxRule, RuleAction, RuleCondition, RuleTrigger } from '@inbox/types'

export interface RuleCreate {
  name: string
  enabled?: boolean
  priority?: number
  trigger: RuleTrigger
  conditions?: RuleCondition[] | null
  actions?: RuleAction[] | null
}

export interface RuleUpdate {
  name?: string
  enabled?: boolean
  priority?: number
  trigger?: RuleTrigger
  conditions?: RuleCondition[] | null
  actions?: RuleAction[] | null
}

export async function listRules(): Promise<InboxRule[]> {
  const { data } = await apiClient.get<InboxRule[]>('/inbox/rules')
  return data
}

export async function createRule(payload: RuleCreate): Promise<InboxRule> {
  const { data } = await apiClient.post<InboxRule>('/inbox/rules', payload)
  return data
}

export async function updateRule(id: number, payload: RuleUpdate): Promise<InboxRule> {
  const { data } = await apiClient.patch<InboxRule>(`/inbox/rules/${id}`, payload)
  return data
}

export async function deleteRule(id: number): Promise<void> {
  await apiClient.delete(`/inbox/rules/${id}`)
}
