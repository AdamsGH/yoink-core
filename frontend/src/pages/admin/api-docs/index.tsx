import { useEffect, useRef, useState } from 'react'
import { useSidebar } from '@core/components/ui/sidebar'

const APP_TOKEN_KEY = 'access_token'

// Scalar uses github-slugger on the API title ("Yoink API" -> "yoink-api")
// to build the localStorage key for persisted auth.
// Key format: scalar-reference-auth-{slug}
// Value format: { secrets: { SchemeName: { type, "x-scalar-secret-token" } }, selected: {...} }
// scalar-client-auth is the active key in Scalar 1.49.x (not the "deprecated" one from main branch).
// Structure: { [nameKey]: { [field]: value } }
// For HTTPBearer: nameKey="httpBearer", token field="token"
const SCALAR_AUTH_KEY = 'scalar-client-auth'

function syncTokenToScalar() {
  const token = localStorage.getItem(APP_TOKEN_KEY)
  if (!token) return
  localStorage.setItem(SCALAR_AUTH_KEY, JSON.stringify({ httpBearer: { token } }))
}

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const iframeRef = useRef<HTMLIFrameElement>(null)
  const [src, setSrc] = useState<string>('')

  useEffect(() => {
    syncTokenToScalar()
    // Timestamp forces iframe reload so Scalar reads fresh localStorage on every visit.
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
