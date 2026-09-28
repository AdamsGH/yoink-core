# Development and database verification

## Prerequisites

Use the Rust toolchain declared by [backend/Cargo.toml](../../backend/Cargo.toml), Node.js 22+, `just`, and Docker where a container check is specified. All Python commands run inside Docker only. Never run host `uv`, `pip`, `python`, or `pytest`.

## Commands

Run Rust workspace commands from the repository root:

| Command | Purpose |
| --- | --- |
| `cd backend && just format` | Apply Rust formatting. |
| `cd backend && just fmt` | Check Rust formatting. |
| `cd backend && just lint` | Clippy all workspace targets with warnings denied. |
| `cd backend && just test` | Workspace tests. |
| `cd backend && just verify-ops` | Workspace checks, Rust 1.88 operator image, and synthetic 41-table import/backup/restore with host/container CLIs and real ENOSPC checks. |
| `cd backend && just verify` | Formatting, lint, Rust and fixture unit tests, smoke checks, recursive script syntax checks, canonical schema generation checks, and documentation link checks. |
| `cd backend && just test-storage-db` | Native storage contracts using private temporary file databases. No PostgreSQL or Docker required. |
| `cd backend && just test-profile-db` | Native profile and credential contracts using private temporary databases. No PostgreSQL or Docker required. |
| `cd backend && just test-db` | Build the Rust server and run native database/process lifecycle contracts: private files, idempotent production CLI migration, exclusive owner refusal, SIGTERM, SIGKILL reopen, file-detach health failure, and shutdown. No PostgreSQL or Docker required. |
| `cd backend && just test-media-fixtures` | Deterministic Node transport and fixture lifecycle contracts without Docker. |
| `cd backend && just check-docs` | Validate the backend documentation links and documented commands. |
| `cd backend && just build-media` | Build the normal opt-in runtime image. |
| `cd backend && just build-media-fixture` | Build a separate fixture image containing test-only controller code. It is not the default production image. |
| `cd backend && just verify-media` | Build both media images, run worker/security checks, check the production worker launcher, then run the full media fixture with an isolated native database volume. |

The media fixture uses native queries throughout and exercises admission, worker execution, durable delivery, restart recovery, cancellation, retries, quotas, access revocation, frozen credential references, cache recovery/expiry and redaction. The source, Bot API, proxy, database controller and container lifecycle helpers live under `backend/scripts/media-fixture/`. Failed scenarios and cleanup failures preserve redacted diagnostics in private files under `backend/target/media-reports/`, independently of disposable fixture cleanup.

## Native persistence operation

The runtime uses native Turso 0.7.2 and one serialized connection. It supports one owning server process per database file, enforced with an advisory sidecar lock. Use a private persistent state directory, not a PostgreSQL URL. Create the directory before container startup and make it writable only by the runtime UID (10001 in the image). For example, use a separately provisioned named volume or a host directory owned by UID 10001 with mode `0700`, mounted at `/var/lib/yoink`. Set `YOINK_RUST_DATABASE_PATH=/var/lib/yoink/fixture.db`.

Normal `serve` and `check-schema` do not create a database or run migrations. Run the explicit `migrate` command first. It creates the database with private permissions if missing and applies pending checksummed migrations transactionally. Repeating it on a valid current schema is safe. `check-schema` validates current history without changing it.

A disposable native fixture never accepts a caller-supplied database path or reads `.env`. It creates a private temporary directory, exercises the actual production CLI, and removes its own fixture. `test-storage-db` and `test-profile-db` avoid PostgreSQL and Docker. No PostgreSQL fixture or host Python is part of these commands.

## Media fixture

The media fixture uses the normal runtime image plus a distinct `media-fixture` image. The latter carries the test-only token-protected loopback controller, shares the native database file with the runtime under test, and is not included in the default production image. The fixture controller binds only to loopback and is not an operator interface. The full media fixture uses its own named native volume; it must not reuse an operator's state. It is opt-in and does not change Compose deployment.

Run `cd backend && just verify-media` to invoke the supported sequence. It builds both images, runs the worker test/security checks, verifies the hardened default launcher, and then runs the complete fixture. The worker tests alone do not establish successful native runtime queue queries. Media behavior, including delivery fencing and retries, must be reported only when the full fixture passes.

## Migration and outstanding gates

The legacy Alembic chain remains offline legacy-catalog tooling and does not run in the native Rust process. [The source schema guide](../../backend/schema/README.md) describes the historical PostgreSQL source catalog. The [PostgreSQL import guide](import.md) documents `just verify-import`, which uses disposable real-Alembic fixtures and a separate importer image. [Stats search verification](stats-search.md) compares the approved Tantivy contract with real PostgreSQL results and tests source/index recovery. [Native backup/restore](backup-restore.md) is verified separately with `just verify-ops`. Full Stats product adapters and production cutover remain separate tasks. No production import, production restore rehearsal, or cutover is claimed.

Never copy a live database file and WAL files as a backup. Use the engine-aware operator utility with its documented ownership and restore constraints. Dataset-scale sizing and production cutover remain unverified.
