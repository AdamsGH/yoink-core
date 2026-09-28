# Architecture

The Rust backend is a modular application with distinct server, HTTP, Telegram, identity, storage, download, execution, and localization crates. Use cases coordinate these modules; the database engine does not collapse their responsibilities into a monolith. This is a parallel implementation, not a completed production replacement or cutover.

## Runtime path

The server optionally starts diagnostics, Telegram, and database-backed services from explicit configuration. Native Turso 0.7.2 is used directly via the `turso` crate. There is no PostgreSQL or SQLx runtime path. Startup opens an existing database, verifies schema compatibility without migration, checks configured worker requirements, and starts adapters. Only explicit `migrate` creates and upgrades database files.

```text
yoink-server
  -> yoink-http             diagnostics
  -> yoink-telegram         owner-private commands and media delivery
       -> yoink-i18n        embedded EN/RU TOML catalogs
  -> yoink-downloads        admission, authorization and worker lifecycle
       -> yoink-identity   read-only legacy-compatible identity snapshots
       -> yoink-storage    durable queue, leases, quotas, cache and delivery journal
       -> yoink-executor   fixed worker process and artifact validation
  -> Python worker          yt-dlp, gallery-dl and bounded ffmpeg processing
```

The media commands are `/download <http(s) URL>`, `/job <UUID>`, and `/cancel <UUID>`. The outer Telegram gate admits only the configured owner in a private chat. The media use case independently checks current identity and grants at admission, before execution, and before each delivery unit.

## Database ownership and lifecycle

One Rust server process owns one local database file at a time. It holds an advisory exclusive lock in a `.yoink-lock` sidecar and serializes operations through one connection. Writes use immediate transactions; read snapshots use deferred transactions. The database parent must already exist and have Unix mode `0700`. Explicit migration creates a missing database with mode `0600`; ordinary startup and schema verification never create or migrate. Unix health checks verify file and lock inode identities as well as parent permissions.

Migration v1 defines three runtime data tables plus the migration ledger. Migration v2 adds the canonical application schema (41 tables) and the latest Stats name-search view. Migration v3 adds Stats search state, a dirty-message queue and source-change triggers. Migration v4 adds explicit indexes for six rowid-alias foreign keys to avoid a pinned-engine enforcement defect. Migration v5 persists acknowledged Telegram progress-message IDs and edit checkpoints. The current database has 48 tables total, including the ledger, plus the Stats view. Migrations are ordered and checksummed. Fingerprints include the schema objects and migration ledger, whose exact DDL is validated. Normal schema verification is read-only and rejects incomplete or drifted state.

## Credentials and security boundary

Identity and settings are consumed as read-only snapshots. Snapshot APIs must not return credential contents. A local embedded database has no PostgreSQL column grants or role ACLs: the trusted server process can read all database contents. The operating-system account and private directory are the security boundary, not per-table privilege isolation. Do not describe this as equivalent to separate PostgreSQL runtime/migration roles.

Admission freezes opaque credential references and versions, not cookie contents or proxy passwords. Execution and each delivery unit recheck current authorization and resolve current valid references. Personal cookies remain distinct from shared pool cookies, which require a current grant. Proxy enablement and current references are checked without silently falling back to direct access. Cache identity includes actor, bot, request parameters, and credential versions. Only complete acknowledged batches are cached as opaque file IDs; cache hits still recheck authorization, credentials, limits, and fenced delivery.

## Guarantees and limitations

- Durable admission, uniqueness/idempotency, queue quotas, bounded retry, expiring worker leases, fencing tokens, heartbeat, cancellation, and cache revision behavior remain storage/use-case contracts.
- Fencing protects database state transitions, not external Telegram effects. Confirmed delivery receipts are retained. Unknown or ambiguous outcomes are terminal and are not automatically resent; exactly-once Telegram delivery is not claimed.
- The fixed worker runs under container read-only/noexec controls and Landlock, with process-group termination, network restrictions, and resource budgets. See [media slice](media-slice.md).
- Worker engines support video/audio processing, playlists, clips, and galleries, but full Python command/settings parity is not complete.
- The operator-only [PostgreSQL importer](import.md) is verified on disposable Alembic fixtures. [Stats search](stats-search.md) uses a separate Tantivy index with an explicitly approved new tokenizer contract. Its repository API is implemented separately from the pending Stats product adapters. The separate [backup/restore utility](backup-restore.md) is verified on isolated data and does not switch deployment. Production cutover remains pending. No production PostgreSQL data has been imported.
