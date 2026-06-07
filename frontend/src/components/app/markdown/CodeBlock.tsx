import { useEffect, useMemo, useState } from 'react'
import AnsiToHtml from 'ansi-to-html'

import { Button } from '@ui'
import { escapeHtml, highlight } from './highlighter'

// Ported verbatim from gateway's frontend/src/components/markdown/CodeBlock.tsx.
// Strategy is layered so cheap cases stay cheap:
//   1. Plain text (no ANSI, language=text)  -> direct <pre>, zero work
//   2. ANSI-coloured terminal output         -> ansi-to-html, no shiki
//   3. Code with a known language            -> shiki async highlight

const ANSI_RE = /\x1b\[[0-9;]*[A-Za-z]/

const ansiConverter = new AnsiToHtml({
  fg: '#e4e4e7',
  bg: 'transparent',
  newline: false,
  escapeXML: true,
  colors: {
    0: '#71717a', 1: '#f87171', 2: '#34d399', 3: '#fbbf24',
    4: '#60a5fa', 5: '#c084fc', 6: '#22d3ee', 7: '#e4e4e7',
    8: '#52525b', 9: '#fca5a5', 10: '#6ee7b7', 11: '#fcd34d',
    12: '#93c5fd', 13: '#d8b4fe', 14: '#67e8f9', 15: '#fafafa',
  },
})

export function CodeBlock({
  code,
  language,
  maxLinesCollapsed = 30,
}: {
  code: string
  language: string
  maxLinesCollapsed?: number
}) {
  const hasAnsi = useMemo(() => ANSI_RE.test(code), [code])
  const lines = useMemo(() => code.split('\n'), [code])
  const [expanded, setExpanded] = useState(false)
  const collapseable = lines.length > maxLinesCollapsed
  const visibleLines = expanded || !collapseable ? lines : lines.slice(0, maxLinesCollapsed)
  const visibleCode = visibleLines.join('\n')

  const [html, setHtml] = useState<string | null>(null)
  const useShiki = !hasAnsi && language !== 'text' && language !== ''

  useEffect(() => {
    if (!useShiki) return
    let cancelled = false
    void (async () => {
      try {
        const r = await highlight(visibleCode, language)
        if (!cancelled) setHtml(r.html)
      } catch {
        if (!cancelled) setHtml(null)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [visibleCode, language, useShiki])

  let body: React.ReactNode
  if (hasAnsi) {
    const rendered = ansiConverter.toHtml(visibleCode)
    body = (
      <pre
        className="shiki-fallback shiki-ansi"
        dangerouslySetInnerHTML={{ __html: `<code>${rendered.replace(/\n/g, '<br/>')}</code>` }}
      />
    )
  } else if (useShiki && html) {
    body = <div className="shiki-host" dangerouslySetInnerHTML={{ __html: html }} />
  } else {
    body = (
      <pre className="shiki-fallback">
        <code dangerouslySetInnerHTML={{ __html: escapeHtml(visibleCode) }} />
      </pre>
    )
  }

  return (
    <div>
      {body}
      {collapseable && (
        <Button
          variant="outline"
          size="sm"
          className="mt-0.5 w-full justify-start font-mono text-xs"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? 'collapse' : `show all ${lines.length} lines`}
        </Button>
      )}
    </div>
  )
}
