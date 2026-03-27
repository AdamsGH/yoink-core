import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiClient } from '@core/lib/api-client'
import { Button } from '@core/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@core/components/ui/card'
import { Input } from '@core/components/ui/input'
import { Label } from '@core/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@core/components/ui/select'
import { toast } from '@core/components/ui/toast'
import TagMapEditor from './TagMapEditor'

const ROLES = ['owner', 'admin', 'moderator', 'user'] as const

export default function AdminBotSettingsPage() {
  const { t } = useTranslation()
  const [settings, setSettings] = useState<Record<string, string | null>>({})
  const [loading, setLoading] = useState(true)
  const [storageChat, setStorageChat] = useState('')
  const [storageThread, setStorageThread] = useState('')
  const [storageSaving, setStorageSaving] = useState(false)

  useEffect(() => {
    apiClient
      .get<Record<string, string | null>>('/bot-settings')
      .then((r) => {
        setSettings(r.data)
        setStorageChat(r.data['inline_storage_chat_id'] ?? '')
        setStorageThread(r.data['inline_storage_thread_id'] ?? '')
      })
      .catch(() => toast.error(t('bot_settings.load_error')))
      .finally(() => setLoading(false))
  }, [])

  const saveStorage = async () => {
    setStorageSaving(true)
    try {
      await apiClient.patch('/bot-settings', {
        inline_storage_chat_id: storageChat || null,
        inline_storage_thread_id: storageThread || null,
      })
      toast.success(t('bot_settings.save_ok'))
    } catch {
      toast.error(t('bot_settings.save_error'))
    } finally {
      setStorageSaving(false)
    }
  }

  const update = async (key: string, value: string) => {
    try {
      await apiClient.patch('/bot-settings', { [key]: value })
      setSettings((prev) => ({ ...prev, [key]: value }))
      toast.success(t('bot_settings.save_ok'))
    } catch {
      toast.error(t('bot_settings.save_error'))
    }
  }

  if (loading) return <div className="flex justify-center py-24 text-muted-foreground">{t('common.loading')}</div>

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader className="px-4 py-3">
          <CardTitle>{t('bot_settings.access_mode')}</CardTitle>
          <CardDescription>{t('bot_settings.access_mode_desc')}</CardDescription>
        </CardHeader>
        <CardContent className="px-4 py-3 space-y-3">
          <div className="space-y-1.5">
            <Label>{t('bot_settings.access_label')}</Label>
            <Select
              value={settings['bot_access_mode'] ?? 'open'}
              onValueChange={(v) => update('bot_access_mode', v)}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="open">{t('bot_settings.access_open')}</SelectItem>
                <SelectItem value="approved_only">{t('bot_settings.access_approved')}</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">{t('bot_settings.access_hint')}</p>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="px-4 py-3">
          <CardTitle>{t('bot_settings.browser_cookies')}</CardTitle>
          <CardDescription>{t('bot_settings.browser_cookies_desc')}</CardDescription>
        </CardHeader>
        <CardContent className="px-4 py-3 space-y-3">
          <div className="space-y-1.5">
            <Label>{t('bot_settings.browser_cookies_role')}</Label>
            <Select
              value={settings['browser_cookies_min_role'] ?? 'owner'}
              onValueChange={(v) => update('browser_cookies_min_role', v)}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {ROLES.map((r) => (
                  <SelectItem key={r} value={r}>
                    {t(`bot_settings.role_${r}` as Parameters<typeof t>[0])}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">{t('bot_settings.browser_cookies_hint')}</p>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex flex-row items-center justify-between px-4 py-3 space-y-0">
          <div>
            <CardTitle>{t('bot_settings.inline_storage')}</CardTitle>
            <CardDescription>{t('bot_settings.inline_storage_desc')}</CardDescription>
          </div>
          <Button onClick={saveStorage} disabled={storageSaving} size="sm">
            {storageSaving ? t('bot_settings.saving') : t('bot_settings.save_storage')}
          </Button>
        </CardHeader>
        <CardContent className="px-4 py-3 space-y-4">
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="storage-chat">{t('bot_settings.storage_chat_id')}</Label>
              <Input
                id="storage-chat"
                placeholder={t('bot_settings.storage_chat_placeholder')}
                value={storageChat}
                onChange={(e) => setStorageChat(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">{t('bot_settings.storage_chat_hint')}</p>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="storage-thread">{t('bot_settings.storage_thread_id')}</Label>
              <Input
                id="storage-thread"
                placeholder={t('bot_settings.storage_thread_placeholder')}
                value={storageThread}
                onChange={(e) => setStorageThread(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">{t('bot_settings.storage_thread_hint')}</p>
            </div>
          </div>
        </CardContent>
      </Card>

      <TagMapEditor />
    </div>
  )
}
