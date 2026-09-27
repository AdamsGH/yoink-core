# 0001: Separate ownership of legacy and Rust database objects

Status: accepted for the parallel implementation, 2026-09-27.

## Context

The Python backend and its plugins share one Alembic chain under `src/db/migrations/versions/`. Users, grants, groups, plugin configuration, media records, and Inbox data already depend on that chain. Recreating those tables in a second migrator would create two competing definitions of the same schema.

Rust also needs durable operation state that does not exist in the legacy schema. Adding it to the existing chain would require coordinating deployment of experimental Rust changes with the working Python service.

## Decision

Alembic remains the only migration authority for the existing `public` tables. Rust reads identity and grant data from explicitly qualified `public` tables. It does not create a second users table or automatically rewrite global roles.

SQLx owns only the new `yoink_runtime` schema. Its migration history lives at `yoink_runtime._sqlx_migrations`. The runtime operation queue lives at `yoink_runtime.operations`. SQL statements qualify application tables with their owning schema.

The `yoink-server migrate` command creates the Rust schema and applies embedded Rust migrations. Normal startup checks the installed versions and checksums without changing them. An absent, dirty, newer, or incompatible history is a startup error, not a request to repair the database automatically.

Database access requires `YOINK_RUST_DATABASE_URL`. Rust does not load `.env`, inherit the Python `database_url` setting, or discover the production database through Compose.

## Consequences

- Rust migrations do not change public business tables. The legacy bootstrap chain remains responsible for fresh installations. Its revision `0009_insight_plugin` now includes the previously missing `InsightUsageLog` baseline, which runtime `create_all()` had masked. Without that DDL, a clean upgrade failed at revision 0042. Already-applied revisions are not rerun, and no production migration was executed.
- Integration fixtures apply the real Alembic chain before testing Rust against it. A hand-written copy of legacy tables is not the compatibility oracle.
- A future deployment needs separate migration and application privileges. The migration principal needs schema creation and DDL rights. Runtime access needs read privileges on the selected legacy identity tables and read/write privileges on Rust operation tables. The current disposable test role is not a production privilege template.
- No rollback command drops Rust tables. Operational rollback stops the Rust service and preserves its schema and queued operations for inspection or later recovery.
- Existing business tables can move to Rust ownership only through a later explicit migration plan. This decision does not authorize production migration or cutover.

## Rejected alternatives

Duplicating users and grants in a Rust-only database would require synchronization before a ban or grant change could take effect. It would also create conflicting identities between the web application and Telegram.

Letting SQLx migrate existing public tables would compete with Alembic. Sharing a database does not require sharing migration ownership.

Putting queue metadata only in Redis would retain the existing database-to-queue publication gap. PostgreSQL stores both admission and claimable work in the same operation row instead.
