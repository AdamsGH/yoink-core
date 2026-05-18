import { useEffect, useState } from 'react'
import { ApiReferenceReact } from '@scalar/api-reference-react'
import '@scalar/api-reference-react/style.css'
import { useSidebar } from '@ui'
import { apiClient } from '@core/lib/api-client'

const APP_TOKEN_KEY = 'access_token'

export default function ApiDocsPage() {
  const { state, isMobile } = useSidebar()
  const [spec, setSpec] = useState<object | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    apiClient
      .get<object>('/_meta/openapi.json')
      .then((r) => setSpec(r.data))
      .catch((e) => setError(e?.response?.status === 403 ? 'Admin role required' : 'Failed to load schema'))
  }, [])

  const left = isMobile
    ? '0px'
    : state === 'collapsed'
      ? 'calc(var(--sidebar-width-icon) + 2px)'
      : 'calc(var(--sidebar-width) + 2px)'

  const bottom = isMobile ? '56px' : '0px'

  const token = localStorage.getItem(APP_TOKEN_KEY) ?? ''

  return (
    <div
      className="fixed top-12 right-0 overflow-auto transition-[left,bottom] duration-200 ease-linear"
      style={{ left, bottom }}
    >
      {error && <div className="p-6 text-sm text-muted-foreground">{error}</div>}
      {!error && !spec && (
        <div className="p-6 text-sm text-muted-foreground">Loading API schema…</div>
      )}
      {spec && (
        <ApiReferenceReact
          configuration={{
            spec: { content: spec },
            theme: 'default',
            hideClientButton: true,
            authentication: {
              preferredSecurityScheme: 'HTTPBearer',
              http: { bearer: { token } },
            },
          }}
        />
      )}
    </div>
  )
}
