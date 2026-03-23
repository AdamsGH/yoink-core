import type { DataProvider } from '@refinedev/core'
import axios from 'axios'

import { apiClient } from './api-client'

function extractMessage(err: unknown): string {
  if (axios.isAxiosError(err)) {
    const data = err.response?.data as { detail?: string; message?: string } | undefined
    return data?.detail ?? data?.message ?? err.message
  }
  return String(err)
}

export const dataProvider: DataProvider = {
  getList: async ({ resource, pagination, filters, sorters }) => {
    const page = pagination?.currentPage ?? 1
    const limit = pagination?.pageSize ?? 20
    const offset = (page - 1) * limit

    const params: Record<string, unknown> = { limit, offset }

    if (filters) {
      for (const f of filters) {
        if ('field' in f) {
          params[f.field] = f.value
        }
      }
    }

    if (sorters && sorters.length > 0) {
      const s = sorters[0]
      if (s) {
        params['sort_by'] = s.field
        params['sort_order'] = s.order
      }
    }

    try {
      const res = await apiClient.get<{ items: unknown[]; total: number }>(
        `/${resource}`,
        { params }
      )
      const { items, total } = res.data
      return { data: items as never[], total }
    } catch (err) {
      return Promise.reject(new Error(extractMessage(err)))
    }
  },

  getOne: async ({ resource, id }) => {
    try {
      const res = await apiClient.get<never>(`/${resource}/${id}`)
      return { data: res.data }
    } catch (err) {
      return Promise.reject(new Error(extractMessage(err)))
    }
  },

  create: async ({ resource, variables }) => {
    try {
      const res = await apiClient.post<never>(`/${resource}`, variables)
      return { data: res.data }
    } catch (err) {
      return Promise.reject(new Error(extractMessage(err)))
    }
  },

  update: async ({ resource, id, variables }) => {
    try {
      const res = await apiClient.patch<never>(`/${resource}/${id}`, variables)
      return { data: res.data }
    } catch (err) {
      return Promise.reject(new Error(extractMessage(err)))
    }
  },

  deleteOne: async ({ resource, id }) => {
    try {
      const res = await apiClient.delete<never>(`/${resource}/${id}`)
      return { data: res.data ?? ({} as never) }
    } catch (err) {
      return Promise.reject(new Error(extractMessage(err)))
    }
  },

  getApiUrl: () => apiClient.defaults.baseURL ?? '/api',
}
