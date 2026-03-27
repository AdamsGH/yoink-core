import { useEffect, useState } from 'react'
import { useGetIdentity } from '@refinedev/core'
import { useTranslation } from 'react-i18next'
import { Download, Music, Film, Package, UserCircle } from 'lucide-react'

import { apiClient } from '../../lib/api-client'
import { formatDate } from '../../lib/utils'
import { useTelegram } from '../../layout/TelegramProvider'
import type { UserStats } from '../../types/plugin'
import { Badge } from '../ui/badge'
import { Button } from '../ui/button'
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from '../ui/sheet'
import { useSidebar } from '../ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip'

const ROLE_VARIANT: Record<string, 'default' | 'secondary' | 'success' | 'warning' | 'destructive' | 'outline'> = {
  owner:      'warning',
  admin:      'default',
  moderator:  'secondary',
  user:       'outline',
  restricted: 'outline',
  banned:     'destructive',
}

const CATEGORY_ICONS: Record<string, React.ReactNode> = {
  video: <Film className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />,
  music: <Music className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />,
  other: <Package className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />,
}

function Avatar({ photoUrl, name, size = 40 }: { photoUrl?: string; name: string; size?: number }) {
  const [error, setError] = useState(false)
  const initials = name.replace(/^@/, '').slice(0, 2).toUpperCase()

  if (photoUrl && !error) {
    return (
      <img
        src={photoUrl}
        alt={name}
        width={size}
        height={size}
        className="rounded-full object-cover shrink-0"
        style={{ width: size, height: size }}
        onError={() => setError(true)}
      />
    )
  }

  return (
    <div
      className="rounded-full bg-muted flex items-center justify-center shrink-0 text-muted-foreground font-semibold select-none"
      style={{ width: size, height: size, fontSize: size * 0.38 }}
    >
      {initials || <UserCircle style={{ width: size * 0.6, height: size * 0.6 }} />}
    </div>
  )
}

function StatCard({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="flex-1 rounded-lg bg-muted px-3 py-2 text-center">
      <p className="text-lg font-bold leading-none">{value}</p>
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

  const categoryEntries = stats?.by_category
    ? Object.entries(stats.by_category).filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1])
    : []

  const avatarOnly = (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon" className="h-8 w-8 rounded-full p-0">
          <Avatar photoUrl={photoUrl} name={name} size={32} />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="right">{name}</TooltipContent>
    </Tooltip>
  )

  const fullTrigger = (
    <Button variant="ghost" className="w-full flex items-center gap-3 rounded-md px-3 py-2 text-left h-auto">
      <Avatar photoUrl={photoUrl} name={name} size={32} />
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
            <Avatar photoUrl={photoUrl} name={name} size={56} />
            <div className="min-w-0">
              <SheetTitle className="text-left text-xl">{name}</SheetTitle>
              {tgUser?.username && (
                <p className="text-sm text-muted-foreground">@{tgUser.username}</p>
              )}
              {role && (
                <Badge variant={ROLE_VARIANT[role] ?? 'outline'} className="mt-1 capitalize">
                  {role}
                </Badge>
              )}
            </div>
          </div>
        </SheetHeader>

        {statsEndpoint && (
          stats ? (
            <div className="space-y-5">
              <div className="flex gap-2">
                <StatCard label={t('userpanel.today', { defaultValue: 'Today' })} value={stats.today} />
                <StatCard label={t('userpanel.this_week', { defaultValue: 'This week' })} value={stats.this_week} />
                <StatCard label={t('userpanel.total', { defaultValue: 'All time' })} value={stats.total} />
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
