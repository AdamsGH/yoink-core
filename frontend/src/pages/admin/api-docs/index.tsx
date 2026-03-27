import { useEffect, useRef } from 'react'
import { useSidebar } from '@core/components/ui/sidebar'

// Scalar stores auth in localStorage under this key.
// Structure: { [schemeName]: { token: string } }
const SCALAR_AUTH_KEY = 'scalar-client-auth'
const APP_TOKEN_KEY = 'access_token'

function syncTokenToScalar() {
  const token = localStorage.getItem(APP_TOKEN_KEY)
  if (!token) return
  // Write into Scalar's auth store so it pre-fills Bearer on load.
  localStorage.setItem(SCALAR_AUTH_KEY, JSON.stringify({ HTTPBearer: { token } }))
}

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const iframeRef = useRef<HTMLIFrameElement>(null)

  // Sync token before iframe loads, and whenever the component mounts.
  useEffect(() => {
    syncTokenToScalar()
  }, [])

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
        ref={iframeRef}
        src="/docs"
        className="h-full w-full border-none"
        title="API Docs"
        allow="clipboard-write"
      />
    </div>
  )
}
