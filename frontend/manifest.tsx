import { BookOpen, Settings2, Shield, Users, UsersRound } from 'lucide-react'

import type { PluginManifest } from '@core/types/plugin'

import AdminUsersPage       from './src/pages/admin/users'
import AdminGroupsPage      from './src/pages/admin/groups/AdminGroupsPage'
import AdminBotSettingsPage from './src/pages/admin/bot-settings'
import ApiDocsPage          from './src/pages/admin/api-docs'

export const corePlugin: PluginManifest = {
  id: 'core',
  name: 'Core',

  routes: [
    { path: '/admin/users',        element: <AdminUsersPage />,       minRole: 'admin' },
    { path: '/admin/groups',       element: <AdminGroupsPage />,      minRole: 'admin' },
    { path: '/admin/bot-settings', element: <AdminBotSettingsPage />, minRole: 'admin' },
    { path: '/admin/api-docs',     element: <ApiDocsPage />,          minRole: 'admin' },
  ],

  navGroups: [
    {
      label: 'Admin',
      i18nKey: 'nav.admin',
      icon: <Shield className="h-4 w-4" />,
      collapsible: true,
      defaultOpen: true,
      minRole: ['owner', 'admin'],
      items: [
        { label: 'Users',        i18nKey: 'nav.users',        path: '/admin/users',        icon: <Users      className="h-4 w-4" />, minRole: ['owner', 'admin'] },
        { label: 'Groups',       i18nKey: 'nav.groups',       path: '/admin/groups',       icon: <UsersRound className="h-4 w-4" />, minRole: ['owner', 'admin'] },
        { label: 'Bot Settings', i18nKey: 'nav.bot_settings', path: '/admin/bot-settings', icon: <Settings2  className="h-4 w-4" />, minRole: ['owner', 'admin'] },
        { label: 'API Docs',     i18nKey: 'nav.api_docs',     path: '/admin/api-docs',     icon: <BookOpen   className="h-4 w-4" />, minRole: ['owner', 'admin'] },
      ],
    },
  ],

  resources: [
    { name: 'users',        list: '/admin/users',        meta: { label: 'Users' } },
    { name: 'groups',       list: '/admin/groups',       meta: { label: 'Groups' } },
    { name: 'bot-settings', list: '/admin/bot-settings', meta: { label: 'Bot Settings' } },
  ],
}
