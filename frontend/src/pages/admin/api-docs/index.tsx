import { useSidebar } from '@core/components/ui/sidebar'

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()

  const left = isMobile
    ? '0px'
    : state === 'collapsed'
      ? 'calc(var(--sidebar-width-icon) + 2px)'
      : 'calc(var(--sidebar-width) + 2px)'

  const bottom = isMobile ? '56px' : '0px'

  return (
    <div
      className="fixed top-12 right-0 transition-[left,bottom] duration-200 ease-linear"
      style={{ left, bottom }}
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
