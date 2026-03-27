import { useEffect, useRef, useState } from 'react'
import { useSidebar } from '@core/components/ui/sidebar'

// Scalar stores auth in localStorage under this key.
// Structure: { [schemeName]: { token: string } }
const SCALAR_AUTH_KEY = 'scalar-client-auth'
const APP_TOKEN_KEY = 'access_token'

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const iframeRef = useRef<HTMLIFrameElement>(null)
  // Start with no src — set it after writing to localStorage so Scalar
  // reads the token on its very first init, not from a stale cached load.
  const [src, setSrc] = useState<string>('')

  useEffect(() => {
    const token = localStorage.getItem(APP_TOKEN_KEY)
    if (token) {
      localStorage.setItem(SCALAR_AUTH_KEY, JSON.stringify({ HTTPBearer: { token } }))
    }
    setSrc('/docs')
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
      {src && (
        <iframe
          ref={iframeRef}
          src={src}
          className="h-full w-full border-none"
          title="API Docs"
          allow="clipboard-write"
        />
      )}
    </div>
  )
}
