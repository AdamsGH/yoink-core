import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Globe, HardDrive, Shield, Tag } from 'lucide-react'

import { plugins } from '@/plugin-registry'
import { usePermissions } from '@/hooks/usePermissions'
import { apiClient } from '@core/lib/api-client'
import { Button } from '@core/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@core/components/ui/card'
import { Input } from '@core/components/ui/input'
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@core/components/ui/select'
import { Separator } from '@core/components/ui/separator'
import { Skeleton } from '@core/components/ui/skeleton'
import { toast } from '@core/components/ui/toast'
import { useTelegramSaveButton, useTelegramWebApp } from '@core/hooks/useTelegramWebApp'
import TagMapEditor from './TagMapEditor'

const ROLES = ['owner', 'admin', 'moderator', 'user'] as const

function SectionSkeleton() {
  return (
    <Card>
      <CardHeader className="px-4 py-3">
        <Skeleton className="h-5 w-36" />
      </CardHeader>
      <CardContent className="px-4 pb-4 space-y-3">
        <Skeleton className="h-4 w-64" />
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-3 w-48" />
      </CardContent>
    </Card>
  )
}

function SettingRow({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col gap-1.5 sm:flex-row sm:items-start sm:gap-4">
      <div className="sm:w-48 shrink-0 pt-0.5">
        <p className="text-sm font-medium leading-none">{label}</p>
        {hint && <p className="mt-1 text-xs text-muted-foreground leading-snug">{hint}</p>}
      </div>
      <div className="flex-1 min-w-0">{children}</div>
    </div>
  )
}

export default function AdminBotSettingsPage() {
  const { t } = useTranslation()
  const { hapticSelection, haptic } = useTelegramWebApp()
  const { hasRole } = usePermissions()

  const pluginSections = plugins
    .flatMap(p => p.botSettingsSections ?? [])
    .filter(s => !s.minRole || hasRole(s.minRole))

  const [settings, setSettings] = useState<Record<string, string | null>>({})
  const [loading, setLoading] = useState(true)

  const [storageChat, setStorageChat] = useState('')
  const [storageThread, setStorageThread] = useState('')
  const [storageSaving, setStorageSaving] = useState(false)
  const [storageDirty, setStorageDirty] = useState(false)
  const originalStorage = useRef({ chat: '', thread: '' })

  useEffect(() => {
    apiClient
      .get<Record<string, string | null>>('/bot-settings')
      .then((r) => {
        setSettings(r.data)
        const chat = r.data['inline_storage_chat_id'] ?? ''
        const thread = r.data['inline_storage_thread_id'] ?? ''
        setStorageChat(chat)
        setStorageThread(thread)
        originalStorage.current = { chat, thread }
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
      originalStorage.current = { chat: storageChat, thread: storageThread }
      setStorageDirty(false)
      haptic('success')
      toast.success(t('bot_settings.save_ok'))
    } catch {
      haptic('error')
      toast.error(t('bot_settings.save_error'))
    } finally {
      setStorageSaving(false)
    }
  }

  useTelegramSaveButton({
    dirty: storageDirty,
    saving: storageSaving,
    text: t('bot_settings.save_storage'),
    onSave: saveStorage,
  })

  const updateStorageChat = (v: string) => {
    setStorageChat(v)
    setStorageDirty(v !== originalStorage.current.chat || storageThread !== originalStorage.current.thread)
  }
  const updateStorageThread = (v: string) => {
    setStorageThread(v)
    setStorageDirty(storageChat !== originalStorage.current.chat || v !== originalStorage.current.thread)
  }

  const update = async (key: string, value: string) => {
    hapticSelection()
    try {
      await apiClient.patch('/bot-settings', { [key]: value })
      setSettings((prev) => ({ ...prev, [key]: value }))
      haptic('success')
      toast.success(t('bot_settings.save_ok'))
    } catch {
      haptic('error')
      toast.error(t('bot_settings.save_error'))
    }
  }

  if (loading) {
    return (
      <div className="space-y-4">
        <SectionSkeleton />
        <SectionSkeleton />
        <SectionSkeleton />
      </div>
    )
  }

  return (
    <div className="space-y-4">

      {/* Access Mode */}
      <Card>
        <CardHeader className="px-4 py-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Globe className="h-4 w-4 text-muted-foreground" />
            {t('bot_settings.access_mode')}
          </CardTitle>
        </CardHeader>
        <CardContent className="px-4 pb-4">
          <SettingRow
            label={t('bot_settings.access_label')}
            hint={t('bot_settings.access_hint')}
          >
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
          </SettingRow>
        </CardContent>
      </Card>

      {/* Browser Cookies */}
      <Card>
        <CardHeader className="px-4 py-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Shield className="h-4 w-4 text-muted-foreground" />
            {t('bot_settings.browser_cookies')}
          </CardTitle>
        </CardHeader>
        <CardContent className="px-4 pb-4">
          <SettingRow
            label={t('bot_settings.browser_cookies_role')}
            hint={t('bot_settings.browser_cookies_hint')}
          >
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
          </SettingRow>
        </CardContent>
      </Card>

      {/* Inline Storage */}
      <Card>
        <CardHeader className="px-4 py-3">
          <div className="flex items-center justify-between gap-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <HardDrive className="h-4 w-4 text-muted-foreground" />
              {t('bot_settings.inline_storage')}
            </CardTitle>
            {storageDirty && (
              <Button
                onClick={saveStorage}
                disabled={storageSaving}
                size="sm"
                className="shrink-0"
              >
                {storageSaving ? t('bot_settings.saving') : t('common.save')}
              </Button>
            )}
          </div>
          <p className="text-xs text-muted-foreground leading-snug mt-1">
            {t('bot_settings.inline_storage_desc')}
          </p>
        </CardHeader>
        <CardContent className="px-4 pb-4 space-y-4">
          <SettingRow
            label={t('bot_settings.storage_chat_id')}
            hint={t('bot_settings.storage_chat_hint')}
          >
            <Input
              placeholder={t('bot_settings.storage_chat_placeholder')}
              value={storageChat}
              onChange={(e) => updateStorageChat(e.target.value)}
            />
          </SettingRow>
          <Separator />
          <SettingRow
            label={t('bot_settings.storage_thread_id')}
            hint={t('bot_settings.storage_thread_hint')}
          >
            <Input
              placeholder={t('bot_settings.storage_thread_placeholder')}
              value={storageThread}
              onChange={(e) => updateStorageThread(e.target.value)}
            />
          </SettingRow>
        </CardContent>
      </Card>

      {/* Tag Map */}
      <Card>
        <CardHeader className="px-4 py-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Tag className="h-4 w-4 text-muted-foreground" />
            {t('tag_map.title')}
          </CardTitle>
          <p className="text-xs text-muted-foreground leading-snug mt-1">
            {t('tag_map.description')}
          </p>
        </CardHeader>
        <CardContent className="px-4 pb-4">
          <TagMapEditor embedded />
        </CardContent>
      </Card>

      {/* Plugin-contributed sections */}
      {pluginSections.map((section, i) => (
        <Card key={i}>
          <CardHeader className="px-4 py-3">
            <CardTitle className="flex items-center gap-2 text-base">
              {section.icon}
              {section.title}
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 pb-4">
            {section.content}
          </CardContent>
        </Card>
      ))}

    </div>
  )
}
