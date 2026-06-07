/**
 * Singleton shiki highlighter shared by every code-block consumer in the
 * yoink miniapp (MarkdownBody for alias prompts, KB pages, anywhere we
 * render LLM/markdown content).
 *
 * - Languages are loaded lazily on first request and cached.
 * - One instance per page; React components await `ensureHighlighter()`
 *   internally and call `highlight()` synchronously after that.
 * - Theme is `github-dark-default`; colours map onto our zinc/emerald
 *   palette closely enough that we don't run a CSS-variable rewrite.
 *
 * Ported verbatim from /opt/docker/llm-stack/gateway/frontend/src/components/
 * markdown/highlighter.ts. Keep changes in lockstep when bumping shiki.
 */

import type { HighlighterCore } from 'shiki/core'
import { createHighlighterCore } from 'shiki/core'
import { createOnigurumaEngine } from 'shiki/engine/oniguruma'

let highlighterPromise: Promise<HighlighterCore> | null = null
const loadedLangs = new Set<string>()
const loadingLangs = new Map<string, Promise<void>>()

const LANG_LOADERS: Record<string, () => Promise<unknown>> = {
  bash: () => import('@shikijs/langs/bash'),
  shell: () => import('@shikijs/langs/shellscript'),
  sh: () => import('@shikijs/langs/shellscript'),
  js: () => import('@shikijs/langs/javascript'),
  javascript: () => import('@shikijs/langs/javascript'),
  ts: () => import('@shikijs/langs/typescript'),
  typescript: () => import('@shikijs/langs/typescript'),
  tsx: () => import('@shikijs/langs/tsx'),
  jsx: () => import('@shikijs/langs/jsx'),
  py: () => import('@shikijs/langs/python'),
  python: () => import('@shikijs/langs/python'),
  json: () => import('@shikijs/langs/json'),
  yaml: () => import('@shikijs/langs/yaml'),
  yml: () => import('@shikijs/langs/yaml'),
  sql: () => import('@shikijs/langs/sql'),
  html: () => import('@shikijs/langs/html'),
  css: () => import('@shikijs/langs/css'),
  md: () => import('@shikijs/langs/markdown'),
  markdown: () => import('@shikijs/langs/markdown'),
  rust: () => import('@shikijs/langs/rust'),
  rs: () => import('@shikijs/langs/rust'),
  go: () => import('@shikijs/langs/go'),
  diff: () => import('@shikijs/langs/diff'),
  dockerfile: () => import('@shikijs/langs/dockerfile'),
  toml: () => import('@shikijs/langs/toml'),
  xml: () => import('@shikijs/langs/xml'),
}

async function ensureHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = (async () => {
      // Theme is imported from shiki's bundled themes; the WASM lives
      // inside @shikijs/engine-oniguruma/wasm-inlined. Both are dynamic
      // so they live in their own lazy chunk.
      const [themeMod, wasmMod] = await Promise.all([
        import('@shikijs/themes/github-dark-default'),
        import('@shikijs/engine-oniguruma/wasm-inlined'),
      ])
      return createHighlighterCore({
        themes: [themeMod.default],
        langs: [],
        engine: await createOnigurumaEngine(wasmMod.default),
      })
    })()
  }
  return highlighterPromise
}

async function ensureLang(lang: string): Promise<string> {
  const lc = lang.toLowerCase()
  const loader = LANG_LOADERS[lc]
  if (!loader) return 'text'
  if (loadedLangs.has(lc)) return lc

  let pending = loadingLangs.get(lc)
  if (!pending) {
    pending = (async () => {
      const hl = await ensureHighlighter()
      const mod = (await loader()) as { default: unknown }
      await hl.loadLanguage(mod.default as Parameters<typeof hl.loadLanguage>[0])
      loadedLangs.add(lc)
    })()
    loadingLangs.set(lc, pending)
  }
  await pending
  loadingLangs.delete(lc)
  return lc
}

export interface HighlightResult {
  html: string
  lang: string
}

/**
 * Returns rendered shiki HTML for the given source. Falls back to a
 * single `<pre><code>` plain block if the language is unknown.
 */
export async function highlight(code: string, lang: string): Promise<HighlightResult> {
  const resolvedLang = await ensureLang(lang)
  const hl = await ensureHighlighter()
  if (resolvedLang === 'text') {
    return {
      lang: 'text',
      html: `<pre class="shiki-fallback"><code>${escapeHtml(code)}</code></pre>`,
    }
  }
  const html = hl.codeToHtml(code, {
    lang: resolvedLang,
    theme: 'github-dark-default',
  })
  return { lang: resolvedLang, html }
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}
