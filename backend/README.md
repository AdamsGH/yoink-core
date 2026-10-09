# Rust backend

Native Rust bot/API with shared Turso 0.7.2 storage and integrated Download, Music, Stats, Insight and Inbox products. Media/content execution uses the container-only confined Python worker. Shared authorization, durable delivery journals, bounded imports, search ownership and cancellation-aware startup/shutdown are verified by library and actual-process acceptance. Production Compose selects this runtime; standalone defaults remain offline. See [deployment](../docs/backend/deployment.md) for source migration, exclusive polling, configuration, backups and rollback.

## Start here

Read the [backend documentation](../docs/backend/index.md) for architecture, configuration, persistence, access control, and development workflows. The [native storage decision](../docs/decisions/0003-native-turso-runtime.md) explains single-process file ownership, migration checks and the changed security boundary.

```sh
cd backend
just verify
just run
```

Defaults are offline: HTTP listens on `127.0.0.1:8088`, Telegram and database access are disabled, and `.env` is not loaded.

```sh
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/api/v1/system
```

For the real media path and its delivery-uncertainty rules, see [the media slice guide](../docs/backend/media-slice.md). The [downloader compatibility matrix](../docs/backend/downloads-parity.md) separates accessible commands, engine capabilities and remaining legacy behavior. `just verify-media` builds separate runtime and test-controller images, then tests the Python worker and full download-to-upload path against a disposable native database volume and a fixture Bot API.

For native database tests, see [development prerequisites and verification](../docs/backend/development.md). `just test-db` uses private temporary files and the real server binary without Docker or a PostgreSQL server. Media tests and all Python work remain inside containers. The separate [PostgreSQL importer](../docs/backend/import.md) is verified against disposable Alembic fixtures, including a real TLS import through its pinned-compiler container. The [backup/restore utility](../docs/backend/backup-restore.md) is verified on isolated data, including container disk exhaustion and process crashes. Source migration requires a stopped-writer import and equivalence verification before activation; isolated candidates are not final cutover evidence.

The [Stats search component](../docs/backend/stats-search.md) uses a separate Tantivy index with durable native change tracking. Its approved tokenizer contract and explicit PostgreSQL differences are covered by a shared corpus. Stats commands, analytics and import routes are integrated into the shared runtime.

The standalone [Turso evaluation](experiments/turso/README.md) and [decision 0002](../docs/decisions/0002-turso-evaluation.md) retain the original feasibility checks. The Rust runtime now uses native Turso. The running Python deployment and its PostgreSQL database are unchanged.

Never start another poller with the production token. Stop the legacy poller before activating native; competing `getUpdates` consumers can consume updates or interrupt service.

### Personal credential controls

Approved, unbanned users can manage only their own non-pool cookies and toggle
only their selected proxy in their own positive private, non-threaded chat.
Operational proxy URL editing and shared cookie pool administration are unchanged.
Personal cookie writes have finite configurable bounds:
`YOINK_RUST_PERSONAL_COOKIE_DOMAINS` (128),
`YOINK_RUST_PERSONAL_COOKIE_ACTOR_BYTES` (67108864, 128 × 512 KiB), and
`YOINK_RUST_PERSONAL_COOKIE_GLOBAL_BYTES` (1073741824, conservative 1 GiB).
These count UTF-8 content bytes, exclude pool rows, and are checked under the
same immediate transaction as canonical-domain replacement and replay watermark.
Domain capacity conservatively counts physical rows (including malformed imports).
Lowering limits never removes imported data or blocks reads/deletes/clear or
non-growing replacements. Rejected growth consumes its message watermark so a
replay cannot install credentials after capacity is freed. No schema change is needed.
`/cookie token` issues a single-use browser-extension sync token. Configuring
`YOINK_RUST_COOKIE_WEB_ORIGIN` adds a WebApp link to the existing `/cookies` page.
No mobile `/cookie-sync` link is emitted because that page is not implemented.
The operator must serve the frontend and API at the configured HTTPS origin.
See [the cookie web contract](docs/cookie-web.md) for the API and verification
boundaries; this is not complete cookie/downloader parity.

Coordinator-only opt-in native acceptance: `node scripts/test-media.mjs personal-controls`
against already built fixture/backend images. This scenario is deliberately not part
of `all`: it exercises a nonowner actual cookie file upload, URL import, owned
cookie extraction, selected proxy on/off routing, callback context denials,
role/ban revocation, immutable replay and secret-free replies on disposable state.
