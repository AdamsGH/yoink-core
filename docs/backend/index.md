# Rust backend

The Rust backend lives in `backend/`. It runs alongside the Python implementation during development. It is not yet a production replacement, and diagnostics do not test downloader or frontend compatibility.

## Develop and operate

- [Development](development.md): prerequisites, verification commands, isolated database fixture, and schema lifecycle.
- [Configuration](configuration.md): opt-in adapters, environment variables, deadlines, and database behavior.

## Understand the implementation

- [Architecture](architecture.md): crate responsibilities and ownership boundaries.
- [Worker protocol](worker-protocol.md): frame bounds, attempt identity, sensitive payloads, and artifact trust boundaries.
- [Persistence](persistence.md): admission, idempotency, leases, cancellation, and recovery.
- [Access control](access-control.md): policy precedence, read-only identity snapshots, and their current limits.
- [Database ownership decision](../decisions/0001-database-ownership.md): ownership of legacy and Rust database objects.

Commands in these pages are relative to the repository root unless they explicitly change directory. These pages describe only the implemented stage: there are no authenticated business endpoints, media worker execution, production cutover, or demonstrated performance gains.
