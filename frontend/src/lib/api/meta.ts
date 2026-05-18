import { apiClient } from '@core/lib/api-client'

/** Endpoints under /_meta - OpenAPI schema, build info, etc. */
export const metaApi = {
  openapi: () => apiClient.get<object>('/_meta/openapi.json'),
}
