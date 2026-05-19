import { useTranslation } from 'react-i18next'

import { formatDate } from '@core/lib/utils'
import type { EffectiveFeatureAccess, UserRole } from '@core/types/api'
import { Badge, Skeleton, Switch } from '@ui'

function AccessBadge({ f }: { f: EffectiveFeatureAccess }) {
  if (!f.effective) {
    return null
  }
  if (f.access_via_role && !f.access_via_grant) {
    return <Badge variant="outline" className="text-[10px] px-1.5 py-0.5 border-blue-500/50 text-blue-600">role</Badge>
  }
  if (f.access_via_grant && f.grant_source === 'tag') {
    return <Badge variant="secondary" className="text-[10px] px-1.5 py-0.5">tag</Badge>
  }
  if (!f.access_via_role && f.access_via_grant) {
    return <Badge variant="outline" className="text-[10px] px-1.5 py-0.5 border-green-500/50 text-green-600">grant</Badge>
  }
  return <Badge variant="outline" className="text-[10px] px-1.5 py-0.5 border-green-500/50 text-green-600">role+grant</Badge>
}

function roleMediaColor(role: UserRole) {
  if (role === 'banned') return 'bg-destructive/10 text-destructive'
  if (role === 'owner') return 'bg-amber-500/10 text-amber-600'
  if (role === 'admin') return 'bg-blue-500/10 text-blue-600'
  if (role === 'moderator') return 'bg-purple-500/10 text-purple-600'
  if (role === 'restricted') return 'bg-orange-500/10 text-orange-600'
  return 'bg-muted text-muted-foreground'
}

export { roleMediaColor }

export function PermissionsTab({
  features,
  permsLoading,
  togglingId,
  onToggle,
}: {
  features: EffectiveFeatureAccess[]
  permsLoading: boolean
  togglingId: string | null
  onToggle: (f: EffectiveFeatureAccess, grant: boolean) => void
}) {
  const { t } = useTranslation()

  const groupedFeatures = features.reduce<Record<string, EffectiveFeatureAccess[]>>((acc, f) => {
    (acc[f.plugin] ??= []).push(f)
    return acc
  }, {})

  return (
    <div className="px-4 py-3 space-y-4 mt-0">
      {permsLoading ? (
        <div className="space-y-3">
          {[0, 1].map((i) => <Skeleton key={i} className="h-16 w-full rounded-md" />)}
        </div>
      ) : features.length === 0 ? (
        <p className="text-sm text-muted-foreground text-center py-4">{t('permissions.no_features')}</p>
      ) : (
        Object.entries(groupedFeatures).map(([plugin, feats]) => (
          <div key={plugin} className="space-y-1.5">
            <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">{plugin}</p>
            <div className="rounded-md border divide-y">
              {feats.map((f) => {
                const key = `${f.plugin}.${f.feature}`
                return (
                  <div key={key} className="px-3 py-2.5 space-y-1">
                    <div className="flex items-center justify-between gap-2">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2 flex-wrap">
                          <p className="text-sm font-medium">{f.label}</p>
                          {f.default_min_role && (
                            <Badge variant="outline" className="text-[10px] px-1.5 py-0.5 font-mono text-muted-foreground">{f.default_min_role}+</Badge>
                          )}
                          <AccessBadge f={f} />
                        </div>
                        {f.description && (
                          <p className="text-xs text-muted-foreground">{f.description}</p>
                        )}
                      </div>
                      <Switch
                        checked={f.effective}
                        disabled={togglingId === key || f.access_via_role || f.grant_source === 'tag'}
                        title={
                          f.access_via_role
                            ? t('permissions.granted_by_role')
                            : f.grant_source === 'tag'
                              ? t('permissions.source_tag_no_revoke')
                              : undefined
                        }
                        onCheckedChange={(v) => onToggle(f, v)}
                      />
                    </div>
                    {f.access_via_grant && f.grant_expires_at && (
                      <p className="text-xs text-muted-foreground pl-0">
                        {t('permissions.expires')}: {formatDate(f.grant_expires_at)}
                      </p>
                    )}
                  </div>
                )
              })}
            </div>
          </div>
        ))
      )}
    </div>
  )
}
