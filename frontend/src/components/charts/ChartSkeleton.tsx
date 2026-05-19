import { Skeleton } from '@ui'
import { StatCardSkeleton } from './StatCard'

export function ChartSkeleton({ height = 120 }: { height?: number }) {
  return <Skeleton className="w-full rounded-md" style={{ height }} />
}

export function SectionSkeleton({ stats = 3, chart = true }: { stats?: number; chart?: boolean }) {
  return (
    <div className="space-y-4">
      <div className={`grid grid-cols-${stats} gap-2`}>
        {Array.from({ length: stats }).map((_, i) => <StatCardSkeleton key={i} />)}
      </div>
      {chart && <ChartSkeleton />}
    </div>
  )
}
