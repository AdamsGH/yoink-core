# Rust backend

Experimental parallel backend on `rewrite/rust-backend`. It is not a production replacement. The current workspace implements diagnostics, shared access policy, read-only legacy identity snapshots, and durable PostgreSQL operation admission and leases. Business adapters and media execution are not connected yet.

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

For the isolated PostgreSQL suite, see [development prerequisites and verification](../docs/backend/development.md). It uses disposable Docker fixtures, not the running application database. All Python work remains inside containers.

Do not share the production bot token with this poller. Competing `getUpdates` consumers can consume updates and interrupt the Python service.
