# Persistence and recovery

The Rust process persists application and operation state in one local Turso 0.7.2 file through `yoink-storage`. It has no PostgreSQL or SQLx dependency at runtime. Schema ownership, file lifecycle, and remaining import gates are summarized in [architecture](architecture.md).

## Ownership, transactions, and schema

A single server process owns a database file, protected by an advisory exclusive sidecar lock. Calls serialize on one connection. Write transactions use `IMMEDIATE`; snapshot reads use `DEFERRED`. The parent directory must already exist and have Unix mode `0700`. Unix database-file creation uses mode `0600`; the sidecar lock is also created privately when needed. Ordinary startup and `check-schema` open an existing database and verify it read-only. Only explicit `migrate` may create a missing file and apply idempotent ordered upgrades.

The migration history records ordered versions and checksums. Current schema validation checks migration checksums, table/object fingerprints including the migration ledger, and exact ledger DDL. V1 introduced three runtime data tables plus the ledger. V2 adds the canonical 41-table application schema and latest Stats name-search view. V3 adds two Stats search bookkeeping tables and source-change triggers. There are 47 tables total including the ledger, plus the Stats view. Unknown versions, changed checksums, drift, or missing migrations fail closed; normal startup does not repair them.

## Open native constraint check

A recovery-test experiment added a table referencing `stats_search_dirty(id)` with `ON DELETE RESTRICT` after opening the search service. Native Turso 0.7.2 accepted deletion of the referenced row, including an explicit `WHERE id = 1`, despite `foreign_keys = 1` and a populated referencing table. The cause is unresolved. Existing canonical-schema cascade and set-null tests pass, but they do not establish restriction enforcement after runtime schema changes. The operations gate must distinguish schema-cache behavior from FK enforcement before any cutover claim. No application table references the derived search queue.

## Admission and idempotency

Admission inserts an operation and its claimable state durably. The uniqueness key includes actor and idempotency key, and an identical repeat returns the existing operation while a differing request conflicts. Telegram source-message idempotency uses bot ID, chat ID, and message ID. It does not make downstream effects exactly once and does not make competing Rust and Python pollers safe to run against one bot.

Keys are non-empty ASCII, at most 128 bytes, and contain no control characters. Global and per-actor pending quotas, attempt counts, and retry delay are bounded by configuration. Retryable worker failures and expired worker leases can return work to the queue while attempts remain.

## Claims, leases, and cancellation

Workers claim queued rows in creation order. A claim increments the attempt and records worker ID, random lease token, and expiry. Heartbeat and state transitions require the matching operation, attempt, worker, and lease token, plus a live lease. Stale claimants cannot change durable state. This fences queue updates, not external effects.

Cancellation is recorded for the owning actor. Queued work is cancelled in storage. During download, cancellation stops and reaps the worker before acknowledging cancellation. Once Telegram upload starts, delivery outcome wins: the service waits for outcome or bounded timeout and does not retry an uncertain upload because cancellation arrived mid-send.

## Delivery state and uncertainty

The queue enters `delivering` before `sendDocument` or another Telegram delivery request. Each delivery unit has a durable journal state before send. A valid success response is recorded with its message IDs. A known rejection is definitive; unrecognized errors, dropped/ambiguous responses, timeouts, crashes, or stale in-flight leases produce terminal `delivery_unknown` and are not automatically resent. Confirmed prefixes remain recorded and are not replayed. Durable fencing cannot make Telegram delivery exactly once.

When `/job <UUID>` reports `delivery_unknown`, check the chat before submitting a new request. A new operation could duplicate a send Telegram accepted when its response was lost.

## Outage recovery and backups

The persistent file preserves admitted operations across process restarts. Queued work and expired leases can be reconciled by later claims. Download execution retries only within configured limits; uncertain delivery is terminal. There is no automatic rollback or drop. Unix health checks detect database or lock-file replacement/removal and parent permission drift, causing the service to fail health rather than silently create a replacement.

Never copy the database file and WAL files while the database is live and treat that as a backup. The separate [PostgreSQL importer](import.md) creates and verifies a new database using a read-only source snapshot. [Stats search](stats-search.md) derives a recoverable Tantivy index from native rows and a transactional dirty queue. Its matching contract deliberately differs from PostgreSQL tokenization. A consistent native backup/restore procedure remains pending. No production data import or cutover has occurred.
