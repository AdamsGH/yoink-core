import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null) return '-'
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i] ?? 'B'}`
}

export function formatDate(iso: string | null | undefined, locale?: string): string {
  if (!iso) return '-'
  return new Date(iso).toLocaleString(locale ?? navigator.language, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** Short date without time: "Jan 2025" */
export function formatDateMonth(iso: string | null | undefined): string {
  if (!iso) return '-'
  return new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'short' })
}

/** Compact date with time, omits year if current: "Mar 29, 10:51" or "Jan 5 2024, 10:51" */
export function formatDateCompact(iso: string | null | undefined): string {
  if (!iso) return '-'
  const d = new Date(iso)
  const now = new Date()
  const sameYear = d.getFullYear() === now.getFullYear()
  return d.toLocaleString(navigator.language, {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** Medium date without time: "Jan 5, 2025" */
export function formatDateDay(iso: string | null | undefined): string {
  if (!iso) return '-'
  return new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
}

export function decodeJwt(token: string): Record<string, unknown> {
  try {
    const payload = token.split('.')[1]
    if (!payload) return {}
    const decoded = atob(payload.replace(/-/g, '+').replace(/_/g, '/'))
    return JSON.parse(decoded) as Record<string, unknown>
  } catch (err) {
    console.error('Failed to decode JWT payload', err)
    return {}
  }
}
