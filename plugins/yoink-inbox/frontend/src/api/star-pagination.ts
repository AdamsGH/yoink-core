import type { GhStarFilters } from './items'
import type { InboxGhStarListResponse, InboxGhStar } from '../types/index'

export function continuation(filters: GhStarFilters, token: string): GhStarFilters {
  const { cursor: _cursor, offset: _offset, ...base } = filters
  return { ...base, ...(filters.sort && filters.sort !== 'starred_at'
    ? { offset: Number(token) } : { cursor: token }) }
}

// Bounded requests, not a bounded result: never silently truncate a catalogue.
export async function collectStars(
  list: (filters: GhStarFilters) => Promise<InboxGhStarListResponse>,
  filters: GhStarFilters = {},
): Promise<InboxGhStar[]> {
  let query = { ...filters, limit: 100 }
  const stars = new Map<number, InboxGhStar>()
  const seen = new Set<string>()
  for (;;) {
    const page = await list(query)
    for (const star of page.items) stars.set(star.id, star)
    if (!page.next_cursor) return [...stars.values()]
    if (seen.has(page.next_cursor)) throw new Error('Repeated star continuation')
    seen.add(page.next_cursor)
    query = continuation(query, page.next_cursor) as typeof query
  }
}
