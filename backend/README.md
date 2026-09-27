# Rust backend

Experimental parallel backend on `rewrite/rust-backend`. It is not a production replacement. The workspace implements diagnostics, shared access policy, read-only legacy identity snapshots, and a durable PostgreSQL queue. Opt-in owner-private Telegram commands use a container-only, Landlock-confined Python media worker. Protocol v2 supports bounded artifact batches, while Rust journals native Telegram delivery units and partial outcomes. Worker engines implement video/audio processing, playlists, clips and galleries. Supported stored preferences and versioned cookie/proxy selection are wired through admission and worker execution. Confirmed deliveries can be reused through the isolated Telegram file-ID cache. Complete command/settings and group/topic parity remain unfinished. Telegram replies use embedded EN/RU TOML catalogs. Other product modules and production cutover are not implemented.

## Start here

Read the [backend documentation](../docs/backend/index.md) for architecture, configuration, persistence, access control, and development workflows. The [database ownership decision](../docs/decisions/0001-database-ownership.md) explains the boundary between Alembic and SQLx.

```sh
cd backend
just verify
just run
```

Defaults are offline: HTTP listens on `127.0.0.1:8088`, Telegram and PostgreSQL are disabled, and `.env` is not loaded.

```sh
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/api/v1/system
```

For the real media path and its delivery-uncertainty rules, see [the media slice guide](../docs/backend/media-slice.md). `just verify-media` builds the experimental image and tests the Python worker and full download-to-upload path against disposable PostgreSQL and a fixture Bot API.

For the isolated PostgreSQL suite, see [development prerequisites and verification](../docs/backend/development.md). It uses disposable Docker fixtures, not the running application database. All Python work remains inside containers.

The standalone [native Turso evaluation](experiments/turso/README.md) tests embedded storage contracts without changing the server or running database. See [decision 0002](../docs/decisions/0002-turso-evaluation.md) for results and migration gates.

Do not share the production bot token with this poller. Competing `getUpdates` consumers can consume updates and interrupt the Python service.
