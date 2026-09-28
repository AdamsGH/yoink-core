# 0001: Separate ownership of legacy and Rust database objects

Status: superseded for the Rust runtime by [decision 0003](0003-native-turso-runtime.md), 2026-09-27. Alembic remains the offline authority for the legacy Python catalog.

## Historical context

The Python backend and plugins share an Alembic chain. The original Rust design added durable operation state to PostgreSQL under a separate `yoink_runtime` schema, with SQLx migrations and role-separated legacy/runtime access. This avoided competing migration ownership for legacy tables.

## Historical decision

The Rust PostgreSQL implementation used `YOINK_RUST_DATABASE_URL`, SQLx-owned `yoink_runtime` migrations, and read-only queries against selected `public` identity/settings tables. It did not authorize production migration or cutover.

That design is no longer the Rust runtime. The current Rust process uses native Turso in a local file and does not use SQLx or PostgreSQL. The Python application's legacy schema and Alembic chain remain separate and are not run by the native Rust server.

## Historical consequences and retained limits

- Alembic remains legacy catalog tooling; it does not manage native Turso schema.
- The former PostgreSQL role split is not a property of the embedded database. The trusted Rust process can read the whole native database. Filesystem ownership and permissions are the security boundary.
- Production cutover, schema import, and data synchronization were not authorized by the historical decision and remain separate pending tasks.

See [decision 0002](0002-turso-evaluation.md) for the historical Turso evaluation and [decision 0003](0003-native-turso-runtime.md) for the current runtime architecture.
