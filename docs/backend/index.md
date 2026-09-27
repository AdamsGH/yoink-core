# Rust backend

The Rust backend lives in `backend/`. It runs alongside the Python implementation during development and is not a production replacement. Its first media slice is opt-in and experimental. Legacy/public behavior remains read-only, and no production cutover has occurred.

## Develop and operate

- [Development](development.md): prerequisites, verification commands, isolated database fixture, and schema lifecycle.
- [Configuration](configuration.md): opt-in adapters, environment variables, deadlines, and database behavior.
- [Media slice guide](media-slice.md): isolated run guidance, restrictions, delivery ambiguity, and container hardening.
- [Database ownership decision](../decisions/0001-database-ownership.md): the boundary between Alembic and SQLx.

## Understand the implementation

- [Architecture](architecture.md): crate responsibilities and ownership boundaries.
- [Worker protocol](worker-protocol.md): frame bounds, attempt identity, sensitive payloads, and artifact trust boundaries.
- [Persistence](persistence.md): admission, idempotency, leases, cancellation, delivery, and recovery.
- [Access control](access-control.md): policy precedence, read-only identity snapshots, and runtime checks.

Commands in these pages are relative to the repository root unless they explicitly change directory. Diagnostics are not a measure of downloader or frontend compatibility, and no performance improvement is claimed.
