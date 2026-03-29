import { useEffect, useRef, useState } from 'react'
import { useSidebar } from '@ui'

const APP_TOKEN_KEY = 'access_token'

function syncTokenToScalar() {
  const token = localStorage.getItem(APP_TOKEN_KEY)
  if (!token) return
  // scalar-client-auth: used by Scalar 1.49.x client panel
  localStorage.setItem('scalar-client-auth', JSON.stringify({ httpBearer: { token } }))
  // scalar-reference-auth-api-1: used by Scalar reference panel.
  // Slug is "api-1" because createApiReference config has no title/slug field.
  localStorage.setItem(
    'scalar-reference-auth-api-1',
    JSON.stringify({
      secrets: {
        HTTPBearer: {
          type: 'http',
          'x-scalar-secret-token': token,
          'x-scalar-secret-username': '',
          'x-scalar-secret-password': '',
        },
      },
      selected: {
        document: { selectedIndex: 0, selectedSchemes: [{ HTTPBearer: [] }] },
        path: null,
      },
    }),
  )
}

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const iframeRef = useRef<HTMLIFrameElement>(null)
  const [src, setSrc] = useState<string>('')

  useEffect(() => {
    syncTokenToScalar()
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
