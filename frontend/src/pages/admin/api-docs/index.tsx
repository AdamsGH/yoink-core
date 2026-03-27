import { useEffect, useRef, useState } from 'react'
import { useSidebar } from '@core/components/ui/sidebar'

const APP_TOKEN_KEY = 'access_token'

function syncTokenToScalar() {
  const token = localStorage.getItem(APP_TOKEN_KEY)
  if (!token) return
  // scalar-client-auth: legacy key used by Scalar 1.49.x client panel
  localStorage.setItem('scalar-client-auth', JSON.stringify({ httpBearer: { token } }))
  // scalar-reference-auth-api-1: new key used by Scalar reference panel.
  // Slug is "api-1" because our createApiReference config has no title/slug field.
  const auth = {
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
  }
  localStorage.setItem('scalar-reference-auth-api-1', JSON.stringify(auth))
}

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const iframeRef = useRef<HTMLIFrameElement>(null)
  const [src, setSrc] = useState<string>('')
  const [debug, setDebug] = useState<Record<string, string | null>>({})

  useEffect(() => {
    syncTokenToScalar()

    // Collect all scalar-* keys for diagnostics
    const info: Record<string, string | null> = {}
    info['access_token (exists)'] = localStorage.getItem(APP_TOKEN_KEY) ? 'YES' : 'NO'
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i)
      if (key && key.startsWith('scalar')) {
        info[key] = localStorage.getItem(key)
      }
    }
    setDebug(info)

    setSrc(`/docs?t=${Date.now()}`)
  }, [])

  const left = isMobile
    ? '0px'
    : state === 'collapsed'
      ? 'calc(var(--sidebar-width-icon) + 2px)'
      : 'calc(var(--sidebar-width) + 2px)'

  return (
    <div className="fixed top-12 right-0 bottom-0 overflow-auto" style={{ left }}>
      <div className="bg-destructive/10 border border-destructive text-xs font-mono p-3 space-y-1">
        <div className="font-bold text-destructive">DEBUG localStorage (remove after fix)</div>
        {Object.entries(debug).map(([k, v]) => (
          <div key={k}>
            <span className="text-muted-foreground">{k}:</span>{' '}
            <span className="break-all">{v ?? 'null'}</span>
          </div>
        ))}
      </div>
      {src && (
        <iframe
          ref={iframeRef}
          src={src}
          className="w-full border-none"
          style={{ height: `calc(100% - ${Object.keys(debug).length * 20 + 48}px)` }}
          title="API Docs"
          allow="clipboard-write"
        />
      )}
    </div>
  )
}
