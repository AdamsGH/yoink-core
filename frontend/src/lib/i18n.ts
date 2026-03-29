/**
 * i18next configuration for the Yoink frontend.
 *
 * Language is initialised from (priority order):
 *   1. localStorage (explicit user choice, fastest)
 *   2. Telegram CloudStorage (synced across devices, Bot API 9.0+)
 *   3. Telegram WebApp languageCode (injected by TelegramProvider)
 *   4. Browser language
 *   5. Fallback: "en"
 *
 * On setLanguage() both localStorage and CloudStorage are written so the
 * preference survives reinstalls and device switches.
 */
import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'

import en from '../locales/en.json'
import ru from '../locales/ru.json'

export const SUPPORTED_LANGUAGES = ['en', 'ru'] as const
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number]

const tgLang = (() => {
  try {
    const tg = window.Telegram?.WebApp
    return tg?.initDataUnsafe?.user?.language_code ?? null
  } catch (err) {
    console.error('Failed to read Telegram language', err)
    return null
  }
})()

const storedLang = (() => {
  try {
    return localStorage.getItem('yoink_lang')
  } catch (err) {
    console.error('Failed to read stored language preference', err)
    return null
  }
})()

const browserLang = navigator.language.split('-')[0]

const detected =
  storedLang ??
  (tgLang && SUPPORTED_LANGUAGES.includes(tgLang as SupportedLanguage) ? tgLang : null) ??
  (SUPPORTED_LANGUAGES.includes(browserLang as SupportedLanguage) ? browserLang : 'en')

i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    ru: { translation: ru },
  },
  lng: detected,
  fallbackLng: 'en',
  interpolation: {
    escapeValue: false,
  },
  react: {
    useSuspense: false,
  },
})

export function setLanguage(lang: SupportedLanguage): void {
  i18n.changeLanguage(lang)
  try {
    localStorage.setItem('yoink_lang', lang)
  } catch {
    // ignore storage errors in restricted contexts
  }
  // CloudStorage persists across reinstalls and devices (Bot API 9.0+)
  try {
    window.Telegram?.WebApp?.CloudStorage?.setItem('lang', lang, () => {})
  } catch {
    // not available outside Telegram - silent
  }
}

/**
 * Read language from Telegram CloudStorage and apply it if different.
 * Call this once after TelegramProvider mounts (async, non-blocking).
 * localStorage takes priority - only syncs if localStorage is empty.
 */
export function syncLangFromCloud(): void {
  try {
    const stored = localStorage.getItem('yoink_lang')
    if (stored) return // localStorage wins
    window.Telegram?.WebApp?.CloudStorage?.getItem('lang', (err, value) => {
      if (!err && value && SUPPORTED_LANGUAGES.includes(value as SupportedLanguage)) {
        i18n.changeLanguage(value)
        try { localStorage.setItem('yoink_lang', value) } catch { /* ignore */ }
      }
    })
  } catch {
    // CloudStorage not available - silent
  }
}

export default i18n
