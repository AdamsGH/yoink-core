# Rust backend

Experimental parallel backend on `rewrite/rust-backend`. It is not a production replacement. The workspace implements diagnostics, shared access policy, read-only identity snapshots and a durable native Turso 0.7.2 queue. Opt-in owner-private Telegram commands use a container-only, Landlock-confined Python media worker. Protocol v2 supports bounded artifact batches, while Rust journals native Telegram delivery units and partial outcomes. Worker engines implement video/audio processing, playlists, clips and galleries. Versioned cookie/proxy selection is wired through admission and worker execution. The Telegram adapter freezes saved mode-specific format settings at admission and supports explicit media-command overrides without changing existing queued requests or same-message replay. Confirmed deliveries can be reused through the isolated Telegram file-ID cache. Durable status-message edits show worker progress and confirmed delivery counts, recover after restart and stop after terminal acknowledgement or lost authorization. Complete command/settings and group/topic parity remain unfinished. Telegram replies use embedded EN/RU TOML catalogs. Other product modules and production cutover are not implemented.

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

For native database tests, see [development prerequisites and verification](../docs/backend/development.md). `just test-db` uses private temporary files and the real server binary without Docker or a PostgreSQL server. Media tests and all Python work remain inside containers. The separate [PostgreSQL importer](../docs/backend/import.md) is verified against disposable Alembic fixtures, including a real TLS import through its pinned-compiler container. The [backup/restore utility](../docs/backend/backup-restore.md) is verified on isolated data, including container disk exhaustion and process crashes. Production cutover remains unfinished. No production data has been imported.

The [Stats search component](../docs/backend/stats-search.md) uses a separate Tantivy index with durable native change tracking. Its approved tokenizer contract and explicit PostgreSQL differences are covered by a shared corpus. Full Stats commands and analytics remain separate product work.

The standalone [Turso evaluation](experiments/turso/README.md) and [decision 0002](../docs/decisions/0002-turso-evaluation.md) retain the original feasibility checks. The Rust runtime now uses native Turso. The running Python deployment and its PostgreSQL database are unchanged.

Do not share the production bot token with this poller. Competing `getUpdates` consumers can consume updates and interrupt the Python service.
