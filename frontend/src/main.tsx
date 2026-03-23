import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import './styles/globals.css'
import './lib/i18n'
import { TelegramProvider } from './layout/TelegramProvider'
import App from './App'

const root = document.getElementById('root')
if (!root) throw new Error('Root element not found')

createRoot(root).render(
  <StrictMode>
    <TelegramProvider>
      <App />
    </TelegramProvider>
  </StrictMode>,
)
