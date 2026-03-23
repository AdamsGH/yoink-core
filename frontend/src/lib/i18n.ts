/**
 * i18next configuration for the Yoink frontend.
 *
 * Language is initialised from:
 *   1. Telegram WebApp's languageCode (injected by TelegramProvider)
 *   2. localStorage (persisted across sessions)
 *   3. Browser language
 *   4. Fallback: "en"
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
    // DeviceStorage (Bot API 9.0) is available via Telegram.WebApp.CloudStorage.
    // It persists across installs and devices, unlike localStorage.
    // Migration path: call CloudStorage.setItem('lang', lang) here when
    // @telegram-apps/sdk exposes a stable DeviceStorage wrapper.
    // For now localStorage is sufficient and zero-latency.
  } catch {
    // ignore storage errors in restricted contexts
  }
}

export default i18n
