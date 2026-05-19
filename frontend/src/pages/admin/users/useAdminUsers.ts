import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { usersApi } from '@core/lib/api'
import type { User, UserUpdateRequest } from '@core/types/api'
import { toast } from '@core/components/ui/toast'

export type StatusFilter = 'all' | 'active' | 'restricted' | 'banned'

export type UserSortField =
  | 'created_at'
  | 'updated_at'
  | 'name'
  | 'role'
  | 'dl_count'
  | 'dl_last_at'

export type UserPeriod = '7' | '30' | '90' | 'all'

export const USERS_PAGE_SIZE = 25

export interface AdminUsersFilters {
  role: string
  status: StatusFilter
  period: UserPeriod
}

export interface UseAdminUsersReturn {
  // data
  items: User[]
  total: number
  totalPages: number
  // load state
  initialLoading: boolean
  fetching: boolean
  // pagination
  page: number
  setPage: (p: number | ((prev: number) => number)) => void
  // search
  search: string
  setSearch: (s: string) => void
  debouncedSearch: string
  // filters
  filters: AdminUsersFilters
  setFilters: (f: AdminUsersFilters | ((prev: AdminUsersFilters) => AdminUsersFilters)) => void
  hasActive: boolean
  resetFilters: () => void
  // sort
  sort: UserSortField
  setSort: (s: UserSortField) => void
  sortDir: 'asc' | 'desc'
  toggleSortDir: () => void
  period: UserPeriod
  setPeriod: (p: UserPeriod) => void
  // drawer
  viewed: User | null
  setViewed: (u: User | null) => void
  handleUpdated: (updated: User) => void
  // actions
  quickBan: (u: User) => Promise<void>
  quickUnban: (u: User) => Promise<void>
}

/**
 * State + handlers for AdminUsersPage. Mirrors useAdminGroups in shape:
 * the page stays declarative (rendering only), all fetching / filtering /
 * sorting state lives here.
 */
export function useAdminUsers(): UseAdminUsersReturn {
  const { t } = useTranslation()

  const [items, setItems] = useState<User[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [initialLoading, setInitialLoading] = useState(true)
  const [fetching, setFetching] = useState(false)

  const [search, setSearch] = useState('')
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [filters, setFilters] = useState<AdminUsersFilters>({ role: 'all', status: 'all', period: '30' })
  const [sort, setSort] = useState<UserSortField>('dl_count')
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('desc')
  const [period, setPeriodState] = useState<UserPeriod>('30')

  const [viewed, setViewed] = useState<User | null>(null)

  const hasActive = debouncedSearch !== '' || filters.role !== 'all' || filters.status !== 'all' || period !== '30'
  const totalPages = Math.max(1, Math.ceil(total / USERS_PAGE_SIZE))

  useEffect(() => {
    const id = setTimeout(() => {
      setDebouncedSearch(search)
      setPage(1)
    }, 300)
    return () => clearTimeout(id)
  }, [search])

  const load = async (
    p: number,
    q: string,
    f: AdminUsersFilters,
    s: UserSortField,
    d: 'asc' | 'desc',
    per: UserPeriod,
  ) => {
    setFetching(true)
    try {
      const res = await usersApi.list({
        offset: (p - 1) * USERS_PAGE_SIZE,
        limit: USERS_PAGE_SIZE,
        sort: s,
        direction: d,
        search: q || undefined,
        role: f.role !== 'all' ? f.role : undefined,
        status: f.status !== 'all' ? f.status : undefined,
        period: per !== 'all' ? per : undefined,
      })
      setItems(res.data.items)
      setTotal(res.data.total)
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setFetching(false)
      setInitialLoading(false)
    }
  }

  useEffect(() => {
    void load(page, debouncedSearch, filters, sort, sortDir, period)
  }, [page, debouncedSearch, filters, sort, sortDir, period])

  const resetFilters = () => {
    setSearch('')
    setDebouncedSearch('')
    setFilters({ role: 'all', status: 'all', period: '30' })
    setPeriodState('30')
    setPage(1)
  }

  const setPeriod = (p: UserPeriod) => {
    setPeriodState(p)
    setPage(1)
  }

  const toggleSortDir = () => {
    setSortDir((d) => (d === 'desc' ? 'asc' : 'desc'))
    setPage(1)
  }

  const quickBan = async (u: User) => {
    try {
      await usersApi.update(u.id, { role: 'banned' } as UserUpdateRequest)
      toast.success(t('users.banned'))
      void load(page, debouncedSearch, filters, sort, sortDir)
    } catch {
      toast.error(t('users.update_error'))
    }
  }

  const quickUnban = async (u: User) => {
    try {
      await usersApi.update(u.id, { role: 'user' } as UserUpdateRequest)
      toast.success(t('users.unbanned'))
      void load(page, debouncedSearch, filters, sort, sortDir)
    } catch {
      toast.error(t('users.update_error'))
    }
  }

  const handleUpdated = (updated: User) => {
    setViewed(updated)
    setItems((prev) => prev.map((u) => (u.id === updated.id ? updated : u)))
  }

  return {
    items,
    total,
    totalPages,
    initialLoading,
    fetching,
    page,
    setPage,
    search,
    setSearch,
    debouncedSearch,
    filters,
    setFilters,
    hasActive,
    resetFilters,
    sort,
    setSort: (s) => {
      setSort(s)
      setPage(1)
    },
    sortDir,
    toggleSortDir,
    period,
    setPeriod,
    viewed,
    setViewed,
    handleUpdated,
    quickBan,
    quickUnban,
  }
}
