import { useEffect, useRef, useState } from 'react'
import { useSidebar } from '@core/components/ui/sidebar'

const SCALAR_AUTH_KEY = 'scalar-client-auth'
const APP_TOKEN_KEY = 'access_token'

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const iframeRef = useRef<HTMLIFrameElement>(null)
  // Cache-bust src so Scalar re-initialises and re-reads localStorage on every visit.
  const [src, setSrc] = useState<string>('')

  useEffect(() => {
    const token = localStorage.getItem(APP_TOKEN_KEY)
    if (token) {
      localStorage.setItem(SCALAR_AUTH_KEY, JSON.stringify({ HTTPBearer: { token } }))
    }
    // Append timestamp to force iframe reload so Scalar reads fresh localStorage.
    setSrc(`/docs?t=${Date.now()}`)
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
