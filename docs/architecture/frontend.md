# Frontend architecture

The Mini App is a React SPA built with Vite and Tailwind. Nginx serves its static assets. Telegram WebApp initData authenticates the user through the native API.

## Core and plugin pages

Core owns authentication, layout, navigation, administration, shared components, and API transport. Plugin manifests contribute routes, navigation groups, and product-specific interfaces.

The runtime resolves imports through [Vite aliases](../../frontend/vite.config.ts). TypeScript paths mirror those aliases for type checking.

| Alias | Source |
| --- | --- |
| `@core` | `frontend/src` |
| `@ui` | Shared shadcn components. |
| `@app` | Application-level components. |
| `@dl` | Downloader frontend. |
| `@stats` | Stats frontend. |
| `@insight` | Insight frontend. |
| `@inbox` | Inbox frontend. |

There is no standalone music frontend bundle. Music account activity appears through shared interfaces.

## API and state

Typed API modules wrap the shared authenticated client. Core modules live under `frontend/src/lib/api`. Plugin API modules live beside their product frontend.

Pages and hooks use those modules rather than constructing transport calls themselves. UI route guards help navigation but do not replace server authorization.

Settings pages use canonical server responses. Downloader settings send changed writable fields with a revision precondition. A conflict preserves the local draft until explicit reload.

## Shared presentation

Core provides application components for setting rows, user panels, empty states, and Markdown. Shared UI components provide dialogs, drawers, selectors, and lists.

English and Russian translations live in the core and plugin locale files. Frontend translation checks compare key coverage.

See [frontend verification](../development/verification.md) and the [downloader settings contract](../features/downloads/settings.md).
