import { useEffect, useState } from 'react'
import { useGetIdentity } from '@refinedev/core'
import { Download, UserCircle } from 'lucide-react'

import { apiClient } from '../../lib/api-client'
import { formatDate } from '../../lib/utils'
import { useTelegram } from '../../layout/TelegramProvider'
import type { UserStats } from '../../types/plugin'
import { Badge } from '../ui/badge'
import { Button } from '../ui/button'
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from '../ui/sheet'

const ROLE_VARIANT: Record<string, 'default' | 'secondary' | 'success' | 'warning' | 'destructive' | 'outline'> = {
  owner:      'warning',
  admin:      'default',
  moderator:  'secondary',
  user:       'outline',
  restricted: 'outline',
  banned:     'destructive',
}

function Avatar({ photoUrl, name, size = 40 }: { photoUrl?: string; name: string; size?: number }) {
  const [error, setError] = useState(false)
  const initials = name.replace(/^@/, '').slice(0, 2).toUpperCase()

  if (photoUrl && !error) {
    // width/height/style are dynamic from size prop
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

  // width/height/fontSize/UserCircle size are dynamic from size prop
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
  const [stats, setStats] = useState<UserStats | null>(null)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    if (!open || !statsEndpoint) return
    apiClient.get<UserStats>(statsEndpoint)
      .then((r) => setStats(r.data))
      .catch(() => {})
  }, [open, statsEndpoint])

  const name = identity?.name ?? tgUser?.first_name ?? '…'
  const role = identity?.role ?? ''
  const photoUrl = tgUser?.photo_url

  const trigger = (
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

  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger asChild>{trigger}</SheetTrigger>
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
                <StatCard label="Today" value={stats.today} />
                <StatCard label="This week" value={stats.this_week} />
                <StatCard label="All time" value={stats.total} />
              </div>

              {stats.top_domains.length > 0 && (
                <div className="space-y-2">
                  <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Top sources</p>
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
                Member since {formatDate(stats.member_since)}
              </p>
            </div>
          ) : (
            <div className="flex justify-center py-8 text-muted-foreground text-sm">Loading…</div>
          )
        )}
      </SheetContent>
    </Sheet>
  )
}
