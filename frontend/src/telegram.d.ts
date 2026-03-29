interface TelegramWebAppUser {
  id: number
  first_name: string
  last_name?: string
  username?: string
  language_code?: string
  /** User's profile photo URL. Available since Bot API 8.1 if privacy settings allow. */
  photo_url?: string
}

interface TelegramBottomButton {
  text: string
  color: string
  textColor: string
  isVisible: boolean
  isActive: boolean
  isProgressVisible: boolean
  hasShineEffect: boolean
  position?: 'left' | 'right' | 'top' | 'bottom'
  setText(text: string): this
  show(): this
  hide(): this
  enable(): this
  disable(): this
  showProgress(leaveActive?: boolean): this
  hideProgress(): this
  setParams(params: {
    text?: string
    color?: string
    text_color?: string
    is_active?: boolean
    is_visible?: boolean
    has_shine_effect?: boolean
    position?: 'left' | 'right' | 'top' | 'bottom'
  }): this
  onClick(cb: () => void): this
  offClick(cb: () => void): this
}

interface TelegramBackButton {
  isVisible: boolean
  show(): this
  hide(): this
  onClick(cb: () => void): this
  offClick(cb: () => void): this
}

interface TelegramHapticFeedback {
  impactOccurred(style: 'light' | 'medium' | 'heavy' | 'rigid' | 'soft'): this
  notificationOccurred(type: 'error' | 'success' | 'warning'): this
  selectionChanged(): this
}

interface TelegramCloudStorage {
  setItem(key: string, value: string, callback?: (err: Error | null, stored: boolean) => void): this
  getItem(key: string, callback: (err: Error | null, value: string) => void): this
  getItems(keys: string[], callback: (err: Error | null, values: Record<string, string>) => void): this
  removeItem(key: string, callback?: (err: Error | null, removed: boolean) => void): this
  getKeys(callback: (err: Error | null, keys: string[]) => void): this
}

interface TelegramWebApp {
  ready: () => void
  expand: () => void
  close: () => void
  initData: string
  initDataUnsafe: {
    user?: TelegramWebAppUser
    start_param?: string
  }
  colorScheme: 'light' | 'dark'
  themeParams: Record<string, string>
  disableVerticalSwipes?: () => void
  enableVerticalSwipes?: () => void
  isExpanded: boolean
  viewportHeight: number
  viewportStableHeight: number

  MainButton: TelegramBottomButton
  SecondaryButton: TelegramBottomButton
  BackButton: TelegramBackButton
  HapticFeedback: TelegramHapticFeedback
  CloudStorage: TelegramCloudStorage

  showAlert(message: string, callback?: () => void): void
  showConfirm(message: string, callback: (confirmed: boolean) => void): void
  showPopup(params: {
    title?: string
    message: string
    buttons?: Array<{ id?: string; type: 'default' | 'ok' | 'close' | 'cancel' | 'destructive'; text?: string }>
  }, callback?: (buttonId: string) => void): void

  enableClosingConfirmation(): void
  disableClosingConfirmation(): void

  openLink(url: string, options?: { try_instant_view?: boolean }): void
  openTelegramLink(url: string): void
}

interface Window {
  Telegram?: {
    WebApp: TelegramWebApp
  }
}
