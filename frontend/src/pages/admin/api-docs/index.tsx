import { useSidebar } from '@core/components/ui/sidebar'

export default function ApiDocsPage() {
  const { state } = useSidebar()
  const sidebarWidth = state === 'collapsed'
    ? 'var(--sidebar-width-icon)'
    : 'var(--sidebar-width)'

  return (
    <div
      className="fixed top-12 bottom-0 right-0 transition-[left] duration-200 ease-linear"
      style={{ left: `calc(${sidebarWidth} + 2px)` }}
    >
      <iframe
        src="/docs"
        className="h-full w-full border-none"
        title="API Docs"
        allow="clipboard-write"
      />
    </div>
  )
}
