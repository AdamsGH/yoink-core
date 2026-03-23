import type { PluginManifest } from './types/plugin'

import { dlPlugin } from '@dl-root/manifest.tsx'
import { statsPlugin } from '@stats-root/manifest.tsx'

export const plugins: PluginManifest[] = [
  dlPlugin,
  statsPlugin,
]
