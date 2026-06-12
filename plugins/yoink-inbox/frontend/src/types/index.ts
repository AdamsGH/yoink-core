// Mirror of the FastAPI Pydantic schemas (api/schemas.py). Keep in sync;
// the API treats these as the wire contract.

export interface InboxItemCategoryRef {
  id: number
  name: string
  slug: string
  color: string | null
  attached_by: 'ai' | 'user' | 'rule'
  confidence: number | null
}

export interface InboxItem {
  id: number
  url: string
  normalized_url: string
  title: string | null
  summary: string | null
  kind: 'link' | 'github_repo' | 'article' | 'video' | 'other'
  source: 'bot' | 'web' | 'api' | 'extension'
  status: 'pending' | 'enriched' | 'classified' | 'archived' | 'failed'
  crawl_status: string | null
  llm_status: 'pending' | 'success' | 'failed' | null
  og_image_url: string | null
  favicon_url: string | null
  author: string | null
  publisher: string | null
  published_at: string | null
  created_at: string
  modified_at: string
  archived_at: string | null
  categories: InboxItemCategoryRef[]
}

export interface InboxItemListResponse {
  items: InboxItem[]
  next_cursor: string | null
}

export interface InboxCategory {
  id: number
  name: string
  slug: string
  icon: string | null
  color: string | null
  kind: 'ai' | 'user' | 'system'
  parent_id: number | null
  description: string | null
  owner_user_id: number
  shared_with_team_id: number | null
  item_count: number
}

export interface InboxGhStar {
  id: number
  gh_repo_id: number
  full_name: string
  owner_login: string
  owner_avatar_url: string | null
  description: string | null
  language: string | null
  topics: string[] | null
  stargazers_count: number
  html_url: string
  homepage: string | null
  archived: boolean
  fork: boolean
  starred_at: string | null
  updated_at: string | null
  ai_labels: string[] | null
  ai_summary: string | null
  can_unstar: boolean
  last_synced_at: string | null
}

export interface InboxGhStarListResponse {
  items: InboxGhStar[]
  next_cursor: string | null
  sync_status: string | null
  last_synced_at: string | null
}
