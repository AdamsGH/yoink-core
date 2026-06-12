import { Inbox, Star, Users, Zap } from 'lucide-react'
import type { PluginManifest } from '@core/types/plugin'

import CategoryPage from './src/pages/CategoryPage'
import InboxPage from './src/pages/InboxPage'
import RulesPage from './src/pages/RulesPage'
import StarFolderPage from './src/pages/StarFolderPage'
import StarsPage from './src/pages/StarsPage'
import TeamsPage from './src/pages/TeamsPage'

export const inboxPlugin: PluginManifest = {
  id: 'inbox',
  name: 'Yoink Inbox',

  routes: [
    { path: '/inbox', element: <InboxPage />, minRole: 'user' },
    { path: '/inbox/stars', element: <StarsPage />, minRole: 'user' },
    { path: '/inbox/rules', element: <RulesPage />, minRole: 'user' },
    { path: '/inbox/teams', element: <TeamsPage />, minRole: 'user' },
    { path: '/inbox/categories/:slug', element: <CategoryPage />, minRole: 'user' },
    { path: '/inbox/folders/:id', element: <StarFolderPage />, minRole: 'user' },
  ],

  navGroups: [
    {
      label: 'Inbox',
      i18nKey: 'nav.inbox',
      icon: <Inbox className="h-4 w-4" />,
      items: [
        { label: 'Items', i18nKey: 'nav.inbox_items', path: '/inbox', icon: <Inbox className="h-4 w-4" />, exact: true },
        { label: 'GitHub Stars', i18nKey: 'nav.inbox_stars', path: '/inbox/stars', icon: <Star className="h-4 w-4" /> },
        { label: 'Rules', i18nKey: 'nav.inbox_rules', path: '/inbox/rules', icon: <Zap className="h-4 w-4" /> },
        { label: 'Teams', i18nKey: 'nav.inbox_teams', path: '/inbox/teams', icon: <Users className="h-4 w-4" /> },
      ],
    },
  ],

  resources: [
    { name: 'inbox-items', list: '/inbox', meta: { label: 'Inbox' } },
    { name: 'inbox-stars', list: '/inbox/stars', meta: { label: 'Stars' } },
    { name: 'inbox-rules', list: '/inbox/rules', meta: { label: 'Rules' } },
    { name: 'inbox-teams', list: '/inbox/teams', meta: { label: 'Teams' } },
  ],
}
