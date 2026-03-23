interface TelegramWebAppUser {
  id: number
  first_name: string
  last_name?: string
  username?: string
  language_code?: string
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
}

interface Window {
  Telegram?: {
    WebApp: TelegramWebApp
  }
}
