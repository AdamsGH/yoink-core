import { apiClient } from '@core/lib/api-client'
import type { UserStats } from '@core/types/plugin'

/**
 * Endpoints scoped to the current authenticated user (/me, /users/me/stats, ...).
 *
 * statsEndpoint is plugin-supplied (PluginManifest.userStatsEndpoint) so
 * different plugins can publish different shapes; the response type is the
 * plugin-side UserStats from @core/types/plugin (NOT the admin UserStats in
 * @core/lib/api/users which has different fields).
 */
export const meApi = {
  getStats: (statsEndpoint: string) =>
    apiClient.get<UserStats>(statsEndpoint),
}
