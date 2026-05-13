import type { ReactNode } from 'react'
import type { ResourceProps } from '@refinedev/core'

export interface NavItem {
  /** If set, item is only visible when user has effective access to this feature.
   *  Format: "plugin:feature", e.g. "insight:summary" */
  requiredFeature?: string
  /** i18n key for the label, e.g. "nav.settings". Falls back to `label`. */
  i18nKey?: string
  label: string
  path: string
  icon: ReactNode
  minRole?: string[]
  /** If true, only exact path match triggers isActive (no startsWith). */
  exact?: boolean
}

export interface NavGroup {
  label?: string
  /** Icon shown when sidebar is collapsed (represents the whole group). */
  icon?: ReactNode
  /** i18n key for the group label. */
  i18nKey?: string
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
  by_category?: Record<string, number>
}

export interface BotSettingsSection {
  /** Section title shown as Card header */
  title: string
  /** Lucide icon node */
  icon?: ReactNode
  /** The content to render inside the Card */
  content: ReactNode
  /** Minimum role required to see this section */
  minRole?: string
}

export interface PluginManifest {
  id: string
  name: string
  routes: PluginRoute[]
  /** Public routes rendered outside AuthGate - no JWT required. */
  navGroups?: NavGroup[]
  resources?: ResourceProps[]
  userStatsEndpoint?: string
  /** Extra cards injected into the admin bot-settings page */
  botSettingsSections?: BotSettingsSection[]
}
