import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import { cn } from '@core/lib/utils'
import { CodeBlock } from './markdown/CodeBlock'

// Markdown renderer for read-only LLM/KB prose: alias prompts, summary
// outputs, anything we display but never let the user edit inline.
// Fenced code blocks route to the same shiki pipeline (./markdown/CodeBlock)
// that the rest of the app uses, so colours stay consistent. Plain inline
// `code` stays as a span.
//
// Ported from /opt/docker/llm-stack/gateway/frontend/src/components/markdown/
// MarkdownBody.tsx. Keep the structure in sync with gateway when shiki
// or react-markdown bumps.
//
// react-markdown 9 dropped the `inline` boolean from the `code` component
// props. Distinguishing inline `code` from a fenced block now relies on
// className: fenced blocks always carry a language-* class (or no class
// but a multi-line value), inline code is a bare <code>. Sniff
// `language-` first; fall back to a multi-line check so language-less
// ``` blocks still route to CodeBlock.
export function MarkdownBody({ text, className }: { text: string; className?: string }) {
  return (
    <div className={cn('md-body text-sm leading-relaxed', className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          code(props: { className?: string; children?: React.ReactNode }) {
            const { className: codeClass, children } = props
            const raw = String(children ?? '')
            const match = /language-(\w+)/.exec(codeClass ?? '')
            const isBlock = !!match || raw.includes('\n')
            if (!isBlock) {
              return (
                <code className="rounded bg-muted px-1 py-0.5 font-mono text-[0.875em] text-primary">
                  {children}
                </code>
              )
            }
            const lang = match?.[1] ?? 'text'
            const value = raw.replace(/\n$/, '')
            return <CodeBlock code={value} language={lang} />
          },
          pre(props: { children?: React.ReactNode }) {
            // The default <pre> wrapper is dropped: CodeBlock renders its own
            // <pre>/shiki <div>; an extra <pre> from react-markdown breaks
            // shiki styling and adds redundant padding.
            return <>{props.children}</>
          },
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  )
}
