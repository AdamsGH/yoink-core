import type { ReactNode } from 'react'
import type { ResourceProps } from '@refinedev/core'

export interface NavItem {
  label: string
  path: string
  icon: ReactNode
  minRole?: string[]
}

export interface NavGroup {
  label?: string
  items: NavItem[]
  minRole?: string[]
  collapsible?: boolean
  defaultOpen?: boolean
}

export interface PluginRoute {
  path: string
  element: ReactNode
  minRole?: string
}

export interface UserStats {
  total: number
  this_week: number
  today: number
  top_domains: Array<{ domain: string; count: number }>
  member_since: string
}

export interface PluginManifest {
  id: string
  name: string
  routes: PluginRoute[]
  navGroups?: NavGroup[]
  resources?: ResourceProps[]
  userStatsEndpoint?: string
}
