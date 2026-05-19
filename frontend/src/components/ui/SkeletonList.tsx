import type { ReactNode } from 'react'

export function SkeletonList({ count, children }: { count: number; children: (i: number) => ReactNode }) {
  return <>{Array.from({ length: count }).map((_, i) => children(i))}</>
}
