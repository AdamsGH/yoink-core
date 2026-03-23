export type UserRole = 'owner' | 'admin' | 'moderator' | 'user' | 'restricted' | 'banned'

export interface User {
  id: number
  username: string | null
  first_name: string | null
  role: UserRole
  theme: string
  created_at: string
  updated_at: string
  ban_until?: string | null
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
  ban_until?: string | null
}
