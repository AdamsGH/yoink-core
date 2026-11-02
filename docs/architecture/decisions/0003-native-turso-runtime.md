# 0003: Native Turso runtime

Status: accepted. This decision defines storage ownership, not permission for a deployment or data migration.

## Decision

The Rust server uses native Turso 0.7.2 through the `turso` crate. It does not use libsql, PostgreSQL, or SQLx at runtime. The application remains modular: server and adapter crates coordinate distinct identity, storage, use-case, execution, HTTP, Telegram, and localization modules.

The native runtime owns one local database file from one process at a time. An advisory exclusive sidecar lock prevents a second owner. A single serialized connection serves operations. Write transactions use immediate behavior and snapshot reads use deferred behavior.

The database parent must already exist and have Unix mode `0700`. The database and sidecar lock files are created with mode `0600`. Health checks verify database and lock inode identities and parent permissions.

Only explicit `migrate` may create a missing database and perform idempotent, checksummed, ordered schema upgrades. Normal startup and `check-schema` require the existing file and verify schema read-only. Verification checks checksummed history, schema fingerprints including migration ledger, and exact ledger DDL. The migration directory defines the current schema. Earlier table totals describe only the original implementation slice.

## Security boundary

Embedded database files do not provide PostgreSQL column-level or role ACLs. The trusted server process can read all database contents. Its operating-system identity, file permissions, and private parent directory form the security boundary. Identity/access snapshots must never return credential values. Do not claim equivalent database privilege isolation to the former PostgreSQL role design.

## Preserved behavior and operational limits

Application contracts retain durable admission, idempotency, quotas, leases, fencing, cancellation, retries, and cache revisions. Delivery journals preserve confirmed prefixes and prohibit automatic resend after ambiguous delivery. Fencing protects durable state, not external Telegram effects. Exactly-once delivery is not claimed.

The default production image excludes the test-only fixture controller. The separate opt-in fixture image contains a bounded, token-protected loopback controller used only by isolated media tests. Database state must be mounted as private writable persistent storage owned by runtime UID 10001. Do not share a live DB file across processes or copy its file and WAL files as a live backup.

Legacy Alembic tooling is retained only as offline legacy-catalog support. The historical PostgreSQL SQL files under `backend/crates/yoink-storage/migrations/` are not loaded by the native runtime, which uses `migrations-turso/`. The separate [PostgreSQL importer](../../operations/import.md) is now verified against disposable real-Alembic fixtures and does not add a PostgreSQL dependency to the server. [Stats search](../../features/stats-search.md) uses a separate Tantivy index with a user-approved new tokenizer contract, not experimental Turso FTS. Native backup/restore and the product services are implemented separately. Their operator and product guides describe current behavior.

## References

- [Backend configuration](../../operations/configuration.md)
- [Backend architecture](../overview.md)
- [Persistence and recovery](../persistence.md)
- [Media slice](../../development/media-fixtures.md)
- [Development and database verification](../../development/verification.md)
