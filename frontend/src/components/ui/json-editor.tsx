import CodeMirror from '@uiw/react-codemirror'
import { json, jsonParseLinter } from '@codemirror/lang-json'
import { linter, lintGutter } from '@codemirror/lint'
import { vscodeDark, vscodeLight } from '@uiw/codemirror-theme-vscode'
import { cn } from '@core/lib/utils'

interface JsonEditorProps {
  value: string
  onChange?: (value: string) => void
  placeholder?: string
  readOnly?: boolean
  minHeight?: string
  maxHeight?: string
  className?: string
  colorScheme?: 'dark' | 'light'
}

const baseExtensions = [json(), lintGutter(), linter(jsonParseLinter())]

export function JsonEditor({
  value,
  onChange,
  placeholder,
  readOnly = false,
  minHeight = '140px',
  maxHeight = '360px',
  className,
  colorScheme = 'dark',
}: JsonEditorProps) {
  return (
    <div
      className={cn('overflow-hidden text-sm relative', className)}
      style={{ minHeight, maxHeight }}
    >
      {!value && placeholder && (
        <pre className="absolute inset-0 px-3 py-2 text-xs font-mono text-muted-foreground/40 pointer-events-none select-none overflow-hidden leading-relaxed z-10 whitespace-pre">
          {placeholder}
        </pre>
      )}
      <CodeMirror
        value={value}
        extensions={baseExtensions}
        theme={colorScheme === 'dark' ? vscodeDark : vscodeLight}
        readOnly={readOnly}
        basicSetup={{
          lineNumbers: true,
          foldGutter: true,
          highlightActiveLine: !readOnly,
          autocompletion: true,
          bracketMatching: true,
          indentOnInput: true,
        }}
        style={{ minHeight, maxHeight }}
        onChange={onChange}
      />
    </div>
  )
}
