import CodeMirror from '@uiw/react-codemirror'
import { json, jsonParseLinter } from '@codemirror/lang-json'
import { linter, lintGutter } from '@codemirror/lint'
import { vscodeDark, vscodeLight } from '@uiw/codemirror-theme-vscode'
import { cn } from '@core/lib/utils'

interface JsonEditorProps {
  value: string
  onChange?: (value: string) => void
  readOnly?: boolean
  minHeight?: string
  maxHeight?: string
  className?: string
  colorScheme?: 'dark' | 'light'
}

const extensions = [json(), lintGutter(), linter(jsonParseLinter())]

export function JsonEditor({
  value,
  onChange,
  readOnly = false,
  minHeight = '140px',
  maxHeight = '360px',
  className,
  colorScheme = 'dark',
}: JsonEditorProps) {
  return (
    <div className={cn('rounded-md border border-border overflow-hidden text-sm', className)}>
      <CodeMirror
        value={value}
        extensions={extensions}
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
