import { KeyRound, Settings2, Users, UsersRound } from 'lucide-react'

import type { PluginManifest } from '@core/types/plugin'

import AdminUsersPage       from './src/pages/admin/users'
import AdminGroupsPage      from './src/pages/admin/groups'
import AdminBotSettingsPage from './src/pages/admin/bot-settings'
import AdminPermissionsPage from './src/pages/admin/permissions'

export const corePlugin: PluginManifest = {
  id: 'core',
  name: 'Core',

  routes: [
    { path: '/admin/users',       element: <AdminUsersPage />,       minRole: 'admin' },
    { path: '/admin/groups',      element: <AdminGroupsPage />,      minRole: 'admin' },
    { path: '/admin/bot-settings', element: <AdminBotSettingsPage />, minRole: 'admin' },
    { path: '/admin/permissions', element: <AdminPermissionsPage />, minRole: 'admin' },
  ],

  navGroups: [
    {
      label: 'Admin',
      collapsible: true,
      defaultOpen: true,
      minRole: ['owner', 'admin'],
      items: [
        { label: 'Users',        path: '/admin/users',        icon: <Users      className="h-4 w-4" />, minRole: ['owner', 'admin'] },
        { label: 'Groups',       path: '/admin/groups',       icon: <UsersRound className="h-4 w-4" />, minRole: ['owner', 'admin'] },
        { label: 'Permissions',  path: '/admin/permissions',  icon: <KeyRound   className="h-4 w-4" />, minRole: ['owner', 'admin'] },
        { label: 'Bot Settings', path: '/admin/bot-settings', icon: <Settings2  className="h-4 w-4" />, minRole: ['owner', 'admin'] },
      ],
    },
  ],

  resources: [
    { name: 'users',        list: '/admin/users',        meta: { label: 'Users' } },
    { name: 'groups',       list: '/admin/groups',       meta: { label: 'Groups' } },
    { name: 'permissions',  list: '/admin/permissions',  meta: { label: 'Permissions' } },
    { name: 'bot-settings', list: '/admin/bot-settings', meta: { label: 'Bot Settings' } },
  ],
}
