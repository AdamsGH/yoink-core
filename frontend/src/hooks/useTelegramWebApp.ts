import { useCallback, useEffect, useRef } from 'react'

function tg() {
  return window.Telegram?.WebApp
}

/**
 * Hook for Telegram WebApp platform APIs.
 *
 * Provides typed wrappers around MainButton, BackButton, HapticFeedback,
 * and closing confirmation. All methods are safe to call outside Telegram
 * (they become no-ops when window.Telegram is unavailable).
 */
export function useTelegramWebApp() {
  // -- HapticFeedback --

  const haptic = useCallback((type: 'success' | 'error' | 'warning') => {
    tg()?.HapticFeedback.notificationOccurred(type)
  }, [])

  const hapticSelection = useCallback(() => {
    tg()?.HapticFeedback.selectionChanged()
  }, [])

  const hapticImpact = useCallback((style: 'light' | 'medium' | 'heavy' = 'light') => {
    tg()?.HapticFeedback.impactOccurred(style)
  }, [])

  // -- MainButton --

  const showMainButton = useCallback((text: string, onClick: () => void) => {
    const btn = tg()?.MainButton
    if (!btn) return
    btn.setText(text).show().enable()
    btn.onClick(onClick)
    return () => {
      btn.offClick(onClick)
      btn.hide()
    }
  }, [])

  const hideMainButton = useCallback(() => {
    tg()?.MainButton.hide()
  }, [])

  const setMainButtonProgress = useCallback((loading: boolean) => {
    const btn = tg()?.MainButton
    if (!btn) return
    if (loading) {
      btn.showProgress(false)
    } else {
      btn.hideProgress()
    }
  }, [])

  // -- BackButton --

  const showBackButton = useCallback((onClick: () => void) => {
    const btn = tg()?.BackButton
    if (!btn) return
    btn.show()
    btn.onClick(onClick)
    return () => {
      btn.offClick(onClick)
      btn.hide()
    }
  }, [])

  // -- Closing confirmation --

  const enableClosingConfirmation = useCallback(() => {
    tg()?.enableClosingConfirmation()
  }, [])

  const disableClosingConfirmation = useCallback(() => {
    tg()?.disableClosingConfirmation()
  }, [])

  // -- Alerts / Confirms --

  const showConfirm = useCallback((message: string): Promise<boolean> => {
    return new Promise((resolve) => {
      const app = tg()
      if (!app) { resolve(true); return }
      app.showConfirm(message, resolve)
    })
  }, [])

  return {
    haptic,
    hapticSelection,
    hapticImpact,
    showMainButton,
    hideMainButton,
    setMainButtonProgress,
    showBackButton,
    enableClosingConfirmation,
    disableClosingConfirmation,
    showConfirm,
    isAvailable: Boolean(tg()),
  }
}

/**
 * Attach a native MainButton for a "save" flow.
 *
 * - Shows the button with `text` when `dirty` is true
 * - Hides it when `dirty` is false
 * - Shows progress while `saving` is true
 * - Calls `onSave` when clicked
 * - Enables closing confirmation while dirty
 */
export function useTelegramSaveButton({
  dirty,
  saving,
  text,
  onSave,
}: {
  dirty: boolean
  saving: boolean
  text: string
  onSave: () => void
}) {
  const { showMainButton, hideMainButton, setMainButtonProgress, enableClosingConfirmation, disableClosingConfirmation } = useTelegramWebApp()
  const cleanupRef = useRef<(() => void) | undefined>(undefined)

  useEffect(() => {
    if (dirty) {
      cleanupRef.current = showMainButton(text, onSave) ?? undefined
      enableClosingConfirmation()
    } else {
      cleanupRef.current?.()
      cleanupRef.current = undefined
      hideMainButton()
      disableClosingConfirmation()
    }
    return () => {
      cleanupRef.current?.()
    }
  }, [dirty, text, onSave, showMainButton, hideMainButton, enableClosingConfirmation, disableClosingConfirmation])

  useEffect(() => {
    setMainButtonProgress(saving)
  }, [saving, setMainButtonProgress])
}
