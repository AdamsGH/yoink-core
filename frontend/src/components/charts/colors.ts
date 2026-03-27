const CTP_VARS = [
  '--ctp-blue', '--ctp-mauve', '--ctp-red', '--ctp-green', '--ctp-peach',
  '--ctp-sky', '--ctp-yellow', '--ctp-pink', '--ctp-teal', '--ctp-lavender',
]
const CTP_FALLBACKS = [
  '#8aadf4', '#c6a0f6', '#ed8796', '#a6da95', '#f5a97f',
  '#91d7e3', '#eed49f', '#f5bde6', '#8bd5ca', '#b7bdf8',
]

let _cache: string[] | null = null

export function chartColors(): string[] {
  if (_cache) return _cache
  const style = getComputedStyle(document.documentElement)
  _cache = CTP_VARS.map((v, i) => style.getPropertyValue(v).trim() || CTP_FALLBACKS[i])
  return _cache
}

/** Invalidate color cache - call when theme changes. */
export function invalidateChartColors(): void {
  _cache = null
}
