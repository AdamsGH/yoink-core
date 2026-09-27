# Architecture

The Rust workspace in [backend/Cargo.toml](../../backend/Cargo.toml) is a parallel implementation stage, not a replacement backend. Current code provides typed access policy, read-only legacy identity snapshots, a PostgreSQL operation queue, and diagnostics adapters. It does not provide authenticated business endpoints or execute media jobs.

## Runtime path

The server binds HTTP diagnostics first and reports readiness only after configured dependencies initialize. Database and Telegram adapters are optional. When configured, database startup verifies the existing Rust schema without modifying it; Telegram initialization prepares the polling adapter. Readiness means these diagnostics dependencies initialized, not that business operations are available.

```text
yoink-server
  -> yoink-http            diagnostics
  -> yoink-telegram        optional polling adapter
  -> yoink-storage         optional SQLx PostgreSQL queue and schema check

Library path, not yet connected to diagnostic adapters:
yoink-identity -> yoink-access
       |
       +-> public identity tables (read only)
```

Domain and application crates provide typed operation and service contracts used by these adapters; the queue does not execute a media worker. See [yoink-access](../../backend/crates/yoink-access/src/lib.rs), [yoink-identity](../../backend/crates/yoink-identity/src/lib.rs), [yoink-storage](../../backend/crates/yoink-storage/src/lib.rs), and [server lifecycle](../../backend/crates/yoink-server/src/runtime.rs).

## Ownership boundaries

- Existing tables in `public` remain owned by the Python service's Alembic chain. Rust reads selected identity/configuration rows, without writing them.
- Rust SQLx migrations own only `yoink_runtime`, including operation admission and lease state. Normal server startup checks schema compatibility and never migrates.
- [Database ownership decision](../decisions/0001-database-ownership.md) is the contract for these boundaries.

This is not authenticated request processing, production cutover, or an end-to-end downloader. No achieved performance improvement is claimed.
