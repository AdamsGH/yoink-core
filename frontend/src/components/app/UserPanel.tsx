import { useEffect, useState } from 'react'
import { useGetIdentity } from '@refinedev/core'
import { useTranslation } from 'react-i18next'
import { Download, Music, Film, Package } from 'lucide-react'

import { apiClient } from '@core/lib/api-client'
import { formatDate } from '@core/lib/utils'
import { useTelegram } from '@core/layout/TelegramProvider'
import type { UserStats } from '@core/types/plugin'
import { Avatar, AvatarFallback, AvatarImage } from '@core/components/ui/avatar'
import { Button } from '@core/components/ui/button'
import { RoleBadge } from '@core/components/app/StatusBadge'
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from '@core/components/ui/sheet'
import { useSidebar } from '@core/components/ui/sidebar'


const CATEGORY_ICONS: Record<string, React.ReactNode> = {
  video: <Film className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />,
  music: <Music className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />,
  other: <Package className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />,
}

function MiniStat({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="flex-1 rounded-lg bg-muted px-3 py-2 text-center">
      <p className="text-lg font-bold leading-none">{typeof value === 'number' ? value.toLocaleString() : value}</p>
      <p className="mt-1 text-xs text-muted-foreground">{label}</p>
    </div>
  )
}

interface UserPanelProps {
  statsEndpoint?: string
}

export function UserPanel({ statsEndpoint }: UserPanelProps) {
  const { data: identity } = useGetIdentity<{ id: string; name: string; role: string }>()
  const { user: tgUser } = useTelegram()
  const { t } = useTranslation()
  const [stats, setStats] = useState<UserStats | null>(null)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    if (!open || !statsEndpoint) return
    apiClient.get<UserStats>(statsEndpoint)
      .then((r) => setStats(r.data))
      .catch(() => {})
  }, [open, statsEndpoint])

  const { state } = useSidebar()
  const collapsed = state === 'collapsed'

  const name = identity?.name ?? tgUser?.first_name ?? '...'
  const role = identity?.role ?? ''
  const photoUrl = tgUser?.photo_url
  const initials = name.replace(/^@/, '').slice(0, 2).toUpperCase() || '?'

  const categoryEntries = stats?.by_category
    ? Object.entries(stats.by_category).filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1])
    : []

  const avatarOnly = (
    <Button variant="ghost" size="icon" className="h-8 w-8 rounded-full p-0">
      <Avatar className="size-8">
        <AvatarImage src={photoUrl} />
        <AvatarFallback className="text-xs">{initials}</AvatarFallback>
      </Avatar>
    </Button>
  )

  const fullTrigger = (
    <Button variant="ghost" className="w-full flex items-center gap-3 rounded-md px-3 py-2 text-left h-auto">
      <Avatar className="size-8">
        <AvatarImage src={photoUrl} />
        <AvatarFallback className="text-xs">{initials}</AvatarFallback>
      </Avatar>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium leading-none">{name}</p>
        {role && (
          <p className="mt-0.5 text-xs text-muted-foreground capitalize">{role}</p>
        )}
      </div>
    </Button>
  )

  const sheetContent = (
    <SheetContent side="bottom" className="rounded-t-xl max-h-[80vh] overflow-y-auto pb-8">
        <SheetHeader className="mb-5">
          <div className="flex items-center gap-4">
            <Avatar className="size-14">
              <AvatarImage src={photoUrl} />
              <AvatarFallback className="text-lg font-semibold">{initials}</AvatarFallback>
            </Avatar>
            <div className="min-w-0">
              <SheetTitle className="text-left text-xl">{name}</SheetTitle>
              {tgUser?.username && (
                <p className="text-sm text-muted-foreground">@{tgUser.username}</p>
              )}
              {role && (
                <RoleBadge role={role as import('@core/types/api').UserRole} className="mt-1" />
              )}
            </div>
          </div>
        </SheetHeader>

        {statsEndpoint && (
          stats ? (
            <div className="space-y-5">
              <div className="flex gap-2">
                <MiniStat label={t('userpanel.today', { defaultValue: 'Today' })} value={stats.today} />
                <MiniStat label={t('userpanel.this_week', { defaultValue: 'This week' })} value={stats.this_week} />
                <MiniStat label={t('userpanel.total', { defaultValue: 'All time' })} value={stats.total} />
              </div>

              {categoryEntries.length > 0 && (
                <div className="space-y-2">
                  <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                    {t('userpanel.by_category', { defaultValue: 'By type' })}
                  </p>
                  <div className="space-y-1">
                    {categoryEntries.map(([cat, count]) => (
                      <div key={cat} className="flex items-center gap-2 text-sm">
                        {CATEGORY_ICONS[cat] ?? <Download className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />}
                        <span className="flex-1 capitalize text-muted-foreground">
                          {t(`userpanel.cat_${cat}`, { defaultValue: cat })}
                        </span>
                        <span className="tabular-nums">{count}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {stats.top_domains.length > 0 && (
                <div className="space-y-2">
                  <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                    {t('userpanel.top_sources', { defaultValue: 'Top sources' })}
                  </p>
                  <div className="space-y-1">
                    {stats.top_domains.map((d) => (
                      <div key={d.domain} className="flex items-center gap-2 text-sm">
                        <Download className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                        <span className="flex-1 truncate">{d.domain}</span>
                        <span className="tabular-nums text-muted-foreground">{d.count}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              <p className="text-xs text-muted-foreground">
                {t('userpanel.member_since', { defaultValue: 'Member since' })} {formatDate(stats.member_since)}
              </p>
            </div>
          ) : (
            <div className="flex justify-center py-8 text-muted-foreground text-sm">
              {t('common.loading')}
            </div>
          )
        )}
      </SheetContent>
  )

  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger asChild>
        {collapsed ? avatarOnly : fullTrigger}
      </SheetTrigger>
      {sheetContent}
    </Sheet>
  )
}
