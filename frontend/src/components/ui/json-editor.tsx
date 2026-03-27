import { useMemo } from 'react'
import CodeMirror, { EditorView } from '@uiw/react-codemirror'
import { json, jsonParseLinter } from '@codemirror/lang-json'
import { linter, lintGutter } from '@codemirror/lint'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { tags as t } from '@lezer/highlight'
import { cn } from '@core/lib/utils'

// Theme that reads CSS variables — stays in sync with catppuccin / dark / light
const appTheme = EditorView.theme(
  {
    '&': {
      color: 'var(--color-foreground)',
      backgroundColor: 'var(--color-card)',
    },
    '.cm-content': {
      caretColor: 'var(--color-foreground)',
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      fontSize: '0.8rem',
      padding: '8px 0',
    },
    '.cm-gutters': {
      backgroundColor: 'var(--color-muted)',
      color: 'var(--color-muted-foreground)',
      border: 'none',
      borderRight: '1px solid var(--color-border)',
    },
    '.cm-activeLineGutter': {
      backgroundColor: 'var(--color-accent)',
    },
    '.cm-activeLine': {
      backgroundColor: 'color-mix(in srgb, var(--color-accent) 50%, transparent)',
    },
    '.cm-selectionBackground, ::selection': {
      backgroundColor: 'color-mix(in srgb, var(--color-primary) 25%, transparent) !important',
    },
    '.cm-cursor': {
      borderLeftColor: 'var(--color-foreground)',
    },
    '.cm-foldPlaceholder': {
      backgroundColor: 'var(--color-muted)',
      border: '1px solid var(--color-border)',
      color: 'var(--color-muted-foreground)',
    },
    '.cm-tooltip': {
      backgroundColor: 'var(--color-popover)',
      color: 'var(--color-popover-foreground)',
      border: '1px solid var(--color-border)',
      borderRadius: '6px',
    },
    '.cm-lintPoint-error:after': {
      borderBottomColor: 'var(--color-destructive)',
    },
    '.cm-diagnostic-error': {
      borderLeft: '3px solid var(--color-destructive)',
      paddingLeft: '6px',
    },
  },
  { dark: true }
)

// Syntax highlighting — mauve/blue/green/peach catppuccin-ish that works on both light/dark
const appHighlight = HighlightStyle.define([
  { tag: t.propertyName,         color: 'var(--ctp-blue,    #89b4fa)' },
  { tag: t.string,               color: 'var(--ctp-green,   #a6e3a1)' },
  { tag: t.number,               color: 'var(--ctp-peach,   #fab387)' },
  { tag: t.bool,                 color: 'var(--ctp-mauve,   #cba6f7)' },
  { tag: t.null,                 color: 'var(--ctp-mauve,   #cba6f7)' },
  { tag: t.punctuation,          color: 'var(--color-muted-foreground)' },
  { tag: t.bracket,              color: 'var(--color-foreground)' },
  { tag: [t.operator, t.separator], color: 'var(--color-muted-foreground)' },
])

interface JsonEditorProps {
  value: string
  onChange?: (value: string) => void
  readOnly?: boolean
  minHeight?: string
  maxHeight?: string
  className?: string
}

export function JsonEditor({
  value,
  onChange,
  readOnly = false,
  minHeight = '140px',
  maxHeight = '360px',
  className,
}: JsonEditorProps) {
  const extensions = useMemo(
    () => [
      json(),
      ...(readOnly
        ? [EditorView.editable.of(false)]
        : [lintGutter(), linter(jsonParseLinter())]
      ),
      syntaxHighlighting(appHighlight),
      appTheme,
      EditorView.lineWrapping,
    ],
    [readOnly]
  )

  return (
    <div
      className={cn('overflow-auto text-sm', className)}
      style={{ minHeight, maxHeight }}
    >
      <CodeMirror
        value={value}
        extensions={extensions}
        theme="none"
        readOnly={readOnly}
        basicSetup={{
          lineNumbers: !readOnly,
          foldGutter: false,
          highlightActiveLine: !readOnly,
          autocompletion: false,
          bracketMatching: !readOnly,
          indentOnInput: !readOnly,
          syntaxHighlighting: false,
        }}
        style={{ minHeight }}
        onChange={onChange}
      />
    </div>
  )
}
