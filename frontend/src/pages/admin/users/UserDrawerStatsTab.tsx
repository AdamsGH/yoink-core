import { useTranslation } from 'react-i18next'

import { formatDate } from '@core/lib/utils'
import type { User } from '@core/types/api'
import { Skeleton } from '@ui'

interface UserStats {
  total: number
  this_week: number
  today: number
  top_domains: { domain: string; count: number }[]
  member_since: string
  dl_last_at: string | null
  music_total: number
  music_last_at: string | null
  ai_total: number
  ai_last_at: string | null
}

function StatCell({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-lg border bg-card p-3 text-center">
      <div className="text-2xl font-bold tabular-nums">{value}</div>
      <div className="text-[11px] text-muted-foreground mt-1 uppercase tracking-wide">{label}</div>
    </div>
  )
}

export function StatsTab({
  user,
  stats,
  statsLoading,
}: {
  user: User
  stats: UserStats | null
  statsLoading: boolean
}) {
  const { t } = useTranslation()

  return (
    <div className="px-4 py-3 space-y-4 mt-0">
      {statsLoading ? (
        <div className="grid grid-cols-3 gap-2">
          {[0, 1, 2].map((i) => <Skeleton key={i} className="h-16 rounded-lg" />)}
        </div>
      ) : stats ? (
        <>
          <div className="grid grid-cols-3 gap-2">
            <StatCell label={t('users.total')} value={stats.total.toLocaleString()} />
            <StatCell label={t('users.this_week')} value={stats.this_week.toLocaleString()} />
            <StatCell label={t('users.today')} value={stats.today.toLocaleString()} />
          </div>
          {stats.top_domains.length > 0 && (
            <div className="space-y-1.5">
              <p className="text-xs text-muted-foreground">{t('users.top_domains')}</p>
              <div className="space-y-1">
                {stats.top_domains.map((d) => (
                  <div key={d.domain} className="flex items-center justify-between text-sm">
                    <span className="font-mono text-xs truncate">{d.domain}</span>
                    <span className="text-muted-foreground ml-2 shrink-0">{d.count}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
          {stats.total === 0 && (
            <p className="text-sm text-muted-foreground text-center py-4">{t('users.no_downloads')}</p>
          )}
          {stats.music_total > 0 && (
            <div className="rounded-md border px-3 py-2 space-y-1">
              <p className="text-xs font-medium">{t('users.music_section', { defaultValue: 'Music' })}</p>
              <div className="flex justify-between text-xs text-muted-foreground">
                <span>{t('users.music_total', { defaultValue: 'Resolved' })}</span>
                <span className="font-medium text-foreground">{stats.music_total.toLocaleString()}</span>
              </div>
              {stats.music_last_at && (
                <div className="flex justify-between text-xs text-muted-foreground">
                  <span>{t('users.music_last', { defaultValue: 'Last used' })}</span>
                  <span>{formatDate(stats.music_last_at)}</span>
                </div>
              )}
            </div>
          )}
          {stats.ai_total > 0 && (
            <div className="rounded-md border px-3 py-2 space-y-1">
              <p className="text-xs font-medium">{t('users.ai_section', { defaultValue: 'AI / Insight' })}</p>
              <div className="flex justify-between text-xs text-muted-foreground">
                <span>{t('users.ai_total', { defaultValue: 'Queries' })}</span>
                <span className="font-medium text-foreground">{stats.ai_total.toLocaleString()}</span>
              </div>
              {stats.ai_last_at && (
                <div className="flex justify-between text-xs text-muted-foreground">
                  <span>{t('users.ai_last', { defaultValue: 'Last used' })}</span>
                  <span>{formatDate(stats.ai_last_at)}</span>
                </div>
              )}
            </div>
          )}
          <div className="text-xs text-muted-foreground border-t pt-3 space-y-1">
            <div className="flex justify-between">
              <span>{t('users.col_joined')}</span>
              <span>{formatDate(user.created_at)}</span>
            </div>
            <div className="flex justify-between">
              <span>{t('users.last_seen')}</span>
              <span>{formatDate(user.updated_at)}</span>
            </div>
            {stats.dl_last_at && (
              <div className="flex justify-between">
                <span>{t('users.dl_last_at', { defaultValue: 'Last download' })}</span>
                <span>{formatDate(stats.dl_last_at)}</span>
              </div>
            )}
          </div>
        </>
      ) : (
        <p className="text-sm text-muted-foreground text-center py-4">{t('common.load_error')}</p>
      )}
    </div>
  )
}

export type { UserStats }
