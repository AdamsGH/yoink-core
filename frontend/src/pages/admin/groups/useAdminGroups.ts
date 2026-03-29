import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiClient } from '@core/lib/api-client'
import type { Group, GroupCreateRequest, GroupUpdateRequest, UserRole } from '@core/types/api'
import { toast } from '@core/components/ui/toast'

export interface EditState {
  group: Group | null
  isNew: boolean
  title: string
  enabled: boolean
  auto_grant_role: UserRole
  allow_pm: boolean
  nsfw_allowed: boolean
  storage_chat_id: string
  storage_thread_id: string
  newId: string
}

export interface UseAdminGroupsReturn {
  items: Group[]
  total: number
  loading: boolean
  edit: EditState | null
  setEdit: (e: EditState | null) => void
  saving: boolean
  load: () => void
  save: () => Promise<void>
}

export function useAdminGroups(): UseAdminGroupsReturn {
  const { t } = useTranslation()
  const [items, setItems] = useState<Group[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const [edit, setEdit] = useState<EditState | null>(null)
  const [saving, setSaving] = useState(false)

  const load = () => {
    setLoading(true)
    apiClient
      .get<Group[]>('/groups')
      .then((res) => { setItems(res.data); setTotal(res.data.length) })
      .catch(() => toast.error(t('common.load_error')))
      .finally(() => setLoading(false))
  }

  useEffect(() => { load() }, [])

  const save = async () => {
    if (!edit) return
    setSaving(true)
    try {
      if (edit.isNew) {
        const body: GroupCreateRequest = {
          id: parseInt(edit.newId, 10),
          title: edit.title || undefined,
          enabled: edit.enabled,
          auto_grant_role: edit.auto_grant_role,
          allow_pm: edit.allow_pm,
          nsfw_allowed: edit.nsfw_allowed,
        }
        await apiClient.post('/groups', body)
        toast.success(t('common.save'))
      } else if (edit.group) {
        const body: GroupUpdateRequest = {
          title: edit.title || undefined,
          enabled: edit.enabled,
          auto_grant_role: edit.auto_grant_role,
          allow_pm: edit.allow_pm,
          nsfw_allowed: edit.nsfw_allowed,
          storage_chat_id: edit.storage_chat_id ? parseInt(edit.storage_chat_id, 10) : null,
          storage_thread_id: edit.storage_thread_id ? parseInt(edit.storage_thread_id, 10) : null,
        }
        await apiClient.patch(`/groups/${edit.group.id}`, body)
        toast.success(t('common.save'))
      }
      setEdit(null)
      load()
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setSaving(false)
    }
  }

  return { items, total, loading, edit, setEdit, saving, load, save }
}
