import type { PluginManifest } from './types/plugin'

import { corePlugin }   from '@core-root/manifest.tsx'
import { dlPlugin }     from '@dl-root/manifest.tsx'
import { statsPlugin }  from '@stats-root/manifest.tsx'
import { insightPlugin } from '@insight-root/manifest.tsx'

export const plugins: PluginManifest[] = [
  corePlugin,
  dlPlugin,
  statsPlugin,
  insightPlugin,
]
