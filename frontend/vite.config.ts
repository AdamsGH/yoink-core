import { fileURLToPath } from 'url'
import { dirname, resolve } from 'path'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import tsconfigPaths from 'vite-tsconfig-paths'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

export default defineConfig({
  base: '/',
  plugins: [tailwindcss(), react(), tsconfigPaths()],

  resolve: {
    alias: {
      '@':           resolve(__dirname, 'src'),
      '@core':       resolve(__dirname, 'src'),
      '@dl':           resolve(__dirname, '../plugins/yoink-dl/frontend/src'),
      '@dl-root':      resolve(__dirname, '../plugins/yoink-dl/frontend'),
      '@stats':        resolve(__dirname, '../plugins/yoink-stats/frontend/src'),
      '@stats-root':   resolve(__dirname, '../plugins/yoink-stats/frontend'),
      '@insight':      resolve(__dirname, '../plugins/yoink-insight/frontend/src'),
      '@insight-root': resolve(__dirname, '../plugins/yoink-insight/frontend'),
    },
  },

  build: {
    outDir: 'dist',
    sourcemap: false,
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks: {
          'vendor-react': ['react', 'react-dom', 'react-router'],
          'vendor-refine': ['@refinedev/core', '@refinedev/react-router'],
          'vendor-recharts': ['recharts'],
        },
      },
    },
  },

  server: {
    host: '0.0.0.0',
    port: 5173,
    fs: {
      allow: ['..'],
    },
    proxy: {
      '/api': { target: 'http://localhost:8000', changeOrigin: true },
    },
  },

  preview: { host: '0.0.0.0', port: 4173 },
})
