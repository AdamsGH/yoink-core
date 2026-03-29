import { fileURLToPath } from 'url'
import { dirname, resolve } from 'path'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

export default defineConfig({
  base: '/',

  // resolve.alias is the single source of truth for all alias resolution at
  // build time. tsconfigPaths is not used — it cannot resolve @core/* from
  // plugin dirs (outside vite root), so alias covers everything.
  // tsconfig.json paths entries stay in sync for tsc / editor support only.
  plugins: [tailwindcss(), react()],

  resolve: {
    alias: {
      '@':             resolve(__dirname, 'src'),
      '@core':         resolve(__dirname, 'src'),
      '@core-root':    resolve(__dirname, '.'),
      '@dl':           resolve(__dirname, '../plugins/yoink-dl/frontend/src'),
      '@dl-root':      resolve(__dirname, '../plugins/yoink-dl/frontend'),
      '@stats':        resolve(__dirname, '../plugins/yoink-stats/frontend/src'),
      '@stats-root':   resolve(__dirname, '../plugins/yoink-stats/frontend'),
      '@insight':      resolve(__dirname, '../plugins/yoink-insight/frontend/src'),
      '@insight-root': resolve(__dirname, '../plugins/yoink-insight/frontend'),
      '@ui':           resolve(__dirname, 'src/components/ui'),
      '@app':          resolve(__dirname, 'src/components/app'),
    },
  },

  build: {
    outDir: 'dist',
    sourcemap: false,
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks: (id) => {
          if (id.includes('@uiw/') || id.includes('@codemirror/')) return 'vendor-codemirror'
          if (id.includes('recharts') || id.includes('d3-')) return 'vendor-recharts'
          if (id.includes('@refinedev/')) return 'vendor-refine'
          if (id.includes('node_modules/react/') || id.includes('node_modules/react-dom/') || id.includes('node_modules/react-router/')) return 'vendor-react'
          return undefined
        },
      },
    },
  },

  server: {
    host: '0.0.0.0',
    port: 5173,
    fs: { allow: ['..'] },
    proxy: {
      '/api': { target: 'http://localhost:8003', changeOrigin: true },
    },
  },

  preview: { host: '0.0.0.0', port: 4173 },
})
