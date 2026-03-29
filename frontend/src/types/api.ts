export type UserRole = 'owner' | 'admin' | 'moderator' | 'user' | 'restricted' | 'banned'

export interface User {
  id: number
  username: string | null
  first_name: string | null
  photo_url: string | null
  role: UserRole
  language: string
  theme: string
  ban_until: string | null
  created_at: string
  updated_at: string
  dl_count: number
  dl_last_at: string | null
}

export interface PaginatedResponse<T> {
  items: T[]
  total: number
  offset: number
  limit: number
  has_more: boolean
}

export interface TokenResponse {
  access_token: string
  token_type: string
  user_id: number
  role: string
}

export interface UserUpdateRequest {
  role?: UserRole
  language?: string
  ban_until?: string | null
}

export interface EffectiveFeatureAccess {
  plugin: string
  feature: string
  label: string
  description: string
  default_min_role: string | null
  access_via_role: boolean
  access_via_grant: boolean
  effective: boolean
  grant_expires_at: string | null
  grant_source: 'manual' | 'tag'
}

export interface ThreadPolicy {
  id: number
  group_id: number
  thread_id: number | null
  name: string | null
  enabled: boolean
}

export interface Group {
  id: number
  title: string | null
  enabled: boolean
  auto_grant_role: UserRole
  allow_pm: boolean
  nsfw_allowed: boolean
  storage_chat_id: number | null
  storage_thread_id: number | null
  photo_url: string | null
  created_at: string
  thread_policies: ThreadPolicy[]
}

export interface GroupCreateRequest {
  id: number
  title?: string
  enabled?: boolean
  auto_grant_role?: UserRole
  allow_pm?: boolean
  nsfw_allowed?: boolean
  storage_chat_id?: number | null
  storage_thread_id?: number | null
}

export interface GroupUpdateRequest {
  title?: string
  enabled?: boolean
  auto_grant_role?: UserRole
  allow_pm?: boolean
  nsfw_allowed?: boolean
  storage_chat_id?: number | null
  storage_thread_id?: number | null
}

export interface Feature {
  plugin: string
  feature: string
  label: string
  description: string
  default_min_role: string | null
}

export interface Permission {
  id: number
  user_id: number
  plugin: string
  feature: string
  granted_by: number
  granted_at: string
  expires_at: string | null
  grant_source: 'manual' | 'tag'
}

export interface GrantPermissionRequest {
  plugin: string
  feature: string
  expires_at?: string | null
}

export interface TagMapEntry {
  tag: string
  features: string[]
}

export interface AvailableFeature {
  key: string
  plugin: string
  feature: string
  label: string
  description: string
}
