# Development and database verification

## Prerequisites

Use the Rust toolchain required by [backend/Cargo.toml](../../backend/Cargo.toml) (declared MSRV 1.88), Node.js 22+, `just`, Docker, and the local images `postgres:16-alpine` and `yoink/yoink:latest`. From the repository root, `just build yoink` builds the latter image without starting services.

The current verification toolchain is Rust/Cargo 1.97.1. The declared MSRV has not been tested. Locked `proc-macro-error2` 2.0.1 emits a future-incompatibility warning for a `proc_macro` re-export (Rust issue #127909). Recheck that dependency before moving to a compiler that rejects it.

All Python commands run inside Docker. Do not run host `uv`, `pip`, `python`, or `pytest`.

## Commands

Run Rust workspace commands from the repository root:

| Command | Purpose |
| --- | --- |
| `cd backend && just fmt` | Check Rust formatting. |
| `cd backend && just lint` | Clippy all workspace targets with warnings denied. |
| `cd backend && just test` | Workspace tests. |
| `cd backend && just verify` | Formatting, lint, unit tests, and non-database smoke checks. |
| `cd backend && just test-db` | Real PostgreSQL/Alembic integration fixture and database process checks. |
| `cd backend && just verify-db` | `verify` plus the database fixture. |

`just build yoink` builds the legacy Docker image used for fixture migrations; it does not bring up the stack.

## What the database fixture does

`backend/scripts/test-db.mjs` uses only already-present images (`--pull never`). It creates one randomly named PostgreSQL container with a generated password, a PostgreSQL data directory on tmpfs, and a randomly assigned host port bound only to loopback. The fixture never reads `.env` or accepts a caller database URL. It applies the real Alembic chain from a read-only bind mount using `yoink/yoink:latest`, then checks that schema inspection rejects a database before explicit Rust migration.

It serially runs the ignored SQLx storage and identity database test suites, runs `migrate` twice and `check-schema`, starts the diagnostic server, and confirms it exits unsuccessfully when the database is stopped. Cleanup removes only the named fixture container and its volumes; it does not remove images, project containers, or source data. Do not run concurrent database fixtures against the same instance.

## Schema lifecycle and troubleshooting

The public schema is migrated by Alembic. SQLx migration history and runtime operations belong to `yoink_runtime`. Run database commands only against a deliberately isolated database URL:

```sh
cd backend
YOINK_RUST_DATABASE_URL='postgresql://user:password@127.0.0.1:5432/isolated' just migrate
YOINK_RUST_DATABASE_URL='postgresql://user:password@127.0.0.1:5432/isolated' just check-schema
```

`migrate` explicitly creates the Rust schema and applies embedded SQLx migrations. `check-schema` and normal startup are read-only: they reject absent, dirty, unexpected, or checksum-mismatched histories rather than repairing them. There is no automatic rollback or drop. On failure, keep the database intact, inspect its ownership/history, and restore a known-good isolated fixture or apply an explicitly reviewed forward migration. Stop the Rust process to roll back application use; preserve queued rows for inspection and recovery. Never point these commands at a production database during this implementation stage.
