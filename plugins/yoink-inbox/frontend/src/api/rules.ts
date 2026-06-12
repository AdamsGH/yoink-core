import { apiClient } from '@core/lib/api-client'

export interface ClassifyHint {
  classify_user_hint: string | null
}

export interface AdminPrompt {
  classify_system_prompt: string | null
  classify_default_prompt: string
}

export async function getClassifyHint(): Promise<ClassifyHint> {
  const { data } = await apiClient.get<ClassifyHint>('/inbox/settings/classify-hint')
  return data
}

export async function setClassifyHint(hint: string | null): Promise<ClassifyHint> {
  const { data } = await apiClient.put<ClassifyHint>('/inbox/settings/classify-hint', { classify_user_hint: hint })
  return data
}

export async function getAdminPrompt(): Promise<AdminPrompt> {
  const { data } = await apiClient.get<AdminPrompt>('/inbox/admin/classify-prompt')
  return data
}

export async function setAdminPrompt(prompt: string | null): Promise<AdminPrompt> {
  const { data } = await apiClient.put<AdminPrompt>('/inbox/admin/classify-prompt', { classify_system_prompt: prompt })
  return data
}

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
