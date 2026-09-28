# 0003: Native Turso runtime

Status: accepted for the Rust runtime; no production data import or cutover authorized.

## Decision

The Rust server uses native Turso 0.7.2 through the `turso` crate. It does not use libsql, PostgreSQL, or SQLx at runtime. The application remains modular: server and adapter crates coordinate distinct identity, storage, use-case, execution, HTTP, Telegram, and localization modules.

The native runtime owns one local database file from one process at a time. An advisory exclusive sidecar lock prevents a second owner. A single serialized connection serves operations. Write transactions use immediate behavior and snapshot reads use deferred behavior. The database parent must already exist and have Unix mode `0700`; the database and sidecar lock files are created with mode `0600`. Health checks verify database and lock inode identities and parent permissions.

Only explicit `migrate` may create a missing database and perform idempotent, checksummed, ordered schema upgrades. Normal startup and `check-schema` require the existing file and verify schema read-only. Verification checks checksummed history, schema fingerprints including migration ledger, and exact ledger DDL. The latest schema comprises three runtime data tables, 41 canonical application tables, and the ledger, for 45 tables total, plus the Stats latest-name view.

## Security boundary

Embedded database files do not provide PostgreSQL column-level or role ACLs. The trusted server process can read all database contents. Its operating-system identity, file permissions, and private parent directory form the security boundary. Identity/access snapshots must never return credential values. Do not claim equivalent database privilege isolation to the former PostgreSQL role design.

## Preserved behavior and operational limits

Durable admission/idempotency, quotas, queue leases and fencing, cancellation, bounded retries, cache revision rules, per-delivery journaling, confirmed-prefix recovery, and no automatic resend for ambiguous delivery remain application contracts. Fencing protects durable state, not external Telegram effects; exactly-once delivery is not claimed.

The default production image excludes the test-only fixture controller. The separate opt-in fixture image contains a bounded, token-protected loopback controller used only by isolated media tests. Database state must be mounted as private writable persistent storage owned by runtime UID 10001. Do not share a live DB file across processes or copy its file and WAL files as a live backup.

Legacy Alembic tooling is retained only as offline legacy-catalog support. The historical PostgreSQL SQL files under `backend/crates/yoink-storage/migrations/` are not loaded by the native runtime, which uses `migrations-turso/`. PostgreSQL data import, Stats full-text compatibility, backup/restore, and production cutover remain pending separate tasks. No data import or cutover is claimed.

## References

- [Backend configuration](../backend/configuration.md)
- [Backend architecture](../backend/architecture.md)
- [Persistence and recovery](../backend/persistence.md)
- [Media slice](../backend/media-slice.md)
- [Development and database verification](../backend/development.md)
