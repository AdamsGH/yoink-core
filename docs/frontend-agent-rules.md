# Frontend changes

These rules cover the core SPA and every `plugins/*/frontend/` directory.

- Preserve `frontend/src/components/ui/` as shadcn-managed source. Do not edit its files.
- Use `EmptyState` from `@app` for empty states, `DividedList` from `@ui` for divided lists and `SkeletonList` from `@ui` for repeated skeleton rows.
- Call typed modules in `lib/api/` or plugin `api/` directories from pages, not `apiClient` directly.
- Import UI components from the `@ui` barrel, not subpaths such as `@ui/badge`.
- Import `IconButton` from `@ui`. Pass the icon as children, not an `icon` prop: `<IconButton tooltip="..." onClick={...}>{icon}</IconButton>`.
- In plugin frontends, import React Router from `react-router`, not `react-router-dom`.

## Frontend conventions

- Alias config lives in `vite.config.ts` only - keep `tsconfig.json` in sync manually (no vite-tsconfig-paths)
- Page files: PascalCase (`AdminGroupsPage.tsx`); heavy pages extract logic into co-located `usePageName.ts` hooks (pattern: `AdminUsersPage` -> `useAdminUsers.ts`, `AdminGroupsPage` -> `useAdminGroups.ts`)
- Large pages with multi-tab drawers split sub-tabs into co-located components (pattern: `UserDrawer.tsx` shell + `BanDatePicker.tsx` + `UserDrawerStatsTab.tsx` + `UserDrawerPermissionsTab.tsx` next to each other, NO barrel `index.ts` re-export)
- Date formatting: use `formatDate` / `formatDateMonth` / `formatDateDay` from `@core/lib/utils` - the project does NOT depend on `date-fns`
- Dev-only diagnostics: gate `console.error` / `console.warn` with `if (import.meta.env.DEV)` so production builds stay quiet
- User confirmations: shadcn `AlertDialog` instead of native `confirm()` / `alert()`, except admin-only surfaces where the native dialog is acceptable (still i18n the text)

### Shared UI components

- `CompactCardHeader` (`@app`) - `px-4 py-3` card header with title + optional actions; skip when the header has extra content below the title row
- `MarkdownBody` (`@app`) - read-only markdown renderer (react-markdown + remark-gfm) for LLM/KB prose. Fenced code blocks route through `@app/markdown/CodeBlock` (shiki with `github-dark-default`, lazy lang loading, ANSI fallback, collapse > 30 lines). Prose styles live under `.md-body` in `globals.css`; ported from gateway's component of the same name, keep in lockstep when bumping shiki.
- `SkeletonList` (`@ui`) - skeleton row repeater: `<SkeletonList count={N}>{(i) => <Row key={i} />}</SkeletonList>`
- `IconButton` supports outline/destructive variants through its `variant` prop.
- `MiniBarChart` from `@core/components/charts` expects the `date` key for XAxis.
