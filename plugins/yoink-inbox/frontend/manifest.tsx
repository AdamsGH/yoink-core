import { Filter, Inbox, Star } from 'lucide-react'
import type { PluginManifest } from '@core/types/plugin'

import InboxPage from './src/pages/InboxPage'
import RulesPage from './src/pages/RulesPage'
import StarsPage from './src/pages/StarsPage'

export const inboxPlugin: PluginManifest = {
  id: 'inbox',
  name: 'Yoink Inbox',

  routes: [
    { path: '/inbox', element: <InboxPage />, minRole: 'user' },
    { path: '/inbox/stars', element: <StarsPage />, minRole: 'user' },
    { path: '/inbox/rules', element: <RulesPage />, minRole: 'user' },
  ],

  navGroups: [
    {
      label: 'Inbox',
      i18nKey: 'nav.inbox',
      icon: <Inbox className="h-4 w-4" />,
      items: [
        { label: 'Items', i18nKey: 'nav.inbox_items', path: '/inbox', icon: <Inbox className="h-4 w-4" /> },
        { label: 'GitHub Stars', i18nKey: 'nav.inbox_stars', path: '/inbox/stars', icon: <Star className="h-4 w-4" /> },
        { label: 'Rules', i18nKey: 'nav.inbox_rules', path: '/inbox/rules', icon: <Filter className="h-4 w-4" /> },
      ],
    },
  ],

  resources: [
    { name: 'inbox-items', list: '/inbox', meta: { label: 'Inbox' } },
    { name: 'inbox-stars', list: '/inbox/stars', meta: { label: 'Stars' } },
    { name: 'inbox-rules', list: '/inbox/rules', meta: { label: 'Rules' } },
  ],
}
