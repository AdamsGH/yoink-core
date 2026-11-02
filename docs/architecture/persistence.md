# Persistence and recovery

The Rust process persists application and operation state in one local Turso 0.7.2 file through `yoink-storage`. It has no PostgreSQL or SQLx dependency at runtime. Schema ownership, file lifecycle, and runtime boundaries are summarized in [architecture](overview.md).

## Ownership, transactions, and schema

A single server process owns a database file, protected by an advisory exclusive sidecar lock. Calls serialize on one connection. Write transactions use `IMMEDIATE`. Snapshot reads use `DEFERRED`.

The parent directory must already exist and have Unix mode `0700`. Unix database-file creation uses mode `0600`. The sidecar lock is also created privately when needed. Ordinary startup and `check-schema` open an existing database and verify it read-only.

Only explicit `migrate` may create a missing file and apply idempotent ordered upgrades.

The migration history records ordered versions and checksums. Current schema validation checks migration checksums, table/object fingerprints including the migration ledger, and exact ledger DDL. Ordered native migrations cover application rows, operation journals, search state, preferences, and product-specific state. The [migration source](../../backend/crates/yoink-storage/migrations-turso/) defines the current schema.

Early migration table totals do not describe the current database. Unknown versions, changed checksums, drift, or missing migrations fail closed. Normal startup does not repair them.

## Native rowid foreign-key workaround

Turso 0.7.2 skips `RESTRICT`/`NO ACTION` checks when the referencing child column is an unindexed `INTEGER PRIMARY KEY` rowid alias. A 48-case [native probe](../../backend/experiments/turso/README.md#foreign-key-restrict-regression-probe) separates this from schema-cache timing: explicit child indexes, ordinary columns, and `INT PRIMARY KEY` work correctly. The engine's unindexed FK scan reads a record column instead of the rowid. Migration V4 adds explicit indexes for the six affected application `user_id` keys.

A V3 upgrade regression checks existing-row preservation, rejected parent updates, and unchanged delete cascades for all six tables. New rowid-alias FKs require an explicit child index. Existing orphan data is not repaired by the migration and fails backup/restore validation. No application table references the derived search queue.

## Admission and idempotency

Admission inserts an operation and its claimable state durably. The uniqueness key includes actor and idempotency key, and an identical repeat returns the existing operation while a differing request conflicts. Telegram source-message idempotency uses bot ID, chat ID, and message ID. It does not make downstream effects exactly once and does not make competing Rust and Python pollers safe to run against one bot.

Keys are non-empty ASCII, at most 128 bytes, and contain no control characters. Global and per-actor pending quotas, attempt counts, and retry delay are bounded by configuration. Retryable worker failures and expired worker leases can return work to the queue while attempts remain.

## Claims, leases, and cancellation

Workers claim queued rows in creation order. A claim increments the attempt and records worker ID, random lease token, and expiry. Heartbeat and state transitions require the matching operation, attempt, worker, and lease token, plus a live lease. Stale claimants cannot change durable state. This fences queue updates, not external effects.

Cancellation is recorded for the owning actor. Queued work is cancelled in storage. During download, cancellation stops and reaps the worker before acknowledging cancellation. Once Telegram upload starts, delivery outcome wins. The service waits for the result or bounded timeout. Mid-send cancellation does not authorize retrying an uncertain upload.

## Delivery state and uncertainty

The queue enters `delivering` before `sendDocument` or another Telegram delivery request. Each delivery unit has a durable journal state before send. A valid success response is recorded with its message IDs. A known rejection is definitive.

Unrecognized errors, dropped/ambiguous responses, timeouts, crashes, or stale in-flight leases produce terminal `delivery_unknown` and are not automatically resent. Confirmed prefixes remain recorded and are not replayed. Durable fencing cannot make Telegram delivery exactly once.

When `/job <UUID>` reports `delivery_unknown`, check the chat before submitting a new request. A new operation could duplicate a send Telegram accepted when its response was lost.

## Outage recovery and backups

The persistent file preserves admitted operations across process restarts. Queued work and expired leases can be reconciled by later claims. Download execution retries only within configured limits. Uncertain delivery is terminal. There is no automatic rollback or drop. Unix health checks detect database or lock-file replacement/removal and parent permission drift, causing the service to fail health rather than silently create a replacement.

Never copy the database file and WAL files while the database is live and treat that as a backup. The separate [PostgreSQL importer](../operations/import.md) creates and verifies a new database using a read-only source snapshot. [Stats search](../features/stats-search.md) derives a recoverable Tantivy index from native rows and a transactional dirty queue. Its matching contract deliberately differs from PostgreSQL tokenization. The [native backup/restore utility](../operations/backup-restore.md) validates and publishes a standalone snapshot to a new path.

Historical restoration can resurrect already-delivered work, so external effects must be reconciled before activation. The active runtime uses native state. The retained PostgreSQL source is not a synchronized rollback target.

### Provider-routing compatibility

Automatic provider routing v1 adds versioned typed engine values to immutable request JSON, not a schema migration. The routing change does not rewrite earlier migration history. A pre-routing binary cannot decode newly admitted `auto_yt_dlp_v1`, `auto_gallery_v1`, or `auto_fallback_v1` jobs, even when its database schema-version check succeeds. These enum values change persisted JSON and typed worker contracts, not the schema version.

The v2 NDJSON envelope alone does not establish worker compatibility. Runtime and worker must be upgraded and rolled back together. Rollback to an older binary requires a pre-routing backup or a compatible reader. Never rewrite frozen routing or delivery journals to force compatibility.

Older `auto` jobs retain their old routing behavior. New automatic cache hashes include the resolved table version and do not reuse old automatic cache identities.
