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

// ---------------------------------------------------------------------------
// Categories (write)
// ---------------------------------------------------------------------------

export interface InboxCategoryCreate {
  name: string
  slug?: string
  icon?: string
  color?: string
  description?: string
  parent_id?: number
  shared_with_team_id?: number
}

// ---------------------------------------------------------------------------
// GH Folders
// ---------------------------------------------------------------------------

export interface InboxGhFolder {
  id: number
  user_id: number
  name: string
  slug: string
  description: string | null
  icon: string | null
  parent_id: number | null
  star_count: number
  gh_list_id: string | null
  gh_list_slug: string | null
  created_at: string
}

export interface InboxGhFolderCreate {
  name: string
  slug?: string
  description?: string
  icon?: string
  parent_id?: number
}

// ---------------------------------------------------------------------------
// Teams
// ---------------------------------------------------------------------------

export interface InboxTeamMember {
  user_id: number
  role: 'owner' | 'admin' | 'member'
  joined_at: string
}

export interface InboxTeam {
  id: number
  name: string
  slug: string
  description: string | null
  owner_user_id: number
  created_at: string
  members: InboxTeamMember[]
}

export interface InboxTeamCreate {
  name: string
  slug?: string
  description?: string
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

export type RuleTrigger = 'item_ingested' | 'item_classified' | 'star_synced'

export interface RuleCondition {
  field: string
  op: string
  value: string
}

export interface RuleAction {
  type: string
  params: Record<string, string>
}

export interface InboxRule {
  id: number
  name: string
  enabled: boolean
  priority: number
  trigger: RuleTrigger
  conditions: RuleCondition[] | null
  actions: RuleAction[] | null
  created_at: string
  modified_at: string
}
