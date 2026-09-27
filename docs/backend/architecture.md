# Architecture

The Rust workspace in [backend/Cargo.toml](../../backend/Cargo.toml) is a parallel implementation, not a production replacement. The opt-in media path connects Telegram commands to identity authorization, PostgreSQL queueing, a confined container Python worker, and durable Telegram delivery. Existing public tables remain read-only. No production cutover has occurred.

## Runtime path

The server binds HTTP diagnostics first and reports readiness only after configured dependencies initialize. PostgreSQL and Telegram are optional unless downloads are enabled. Startup verifies schema compatibility without migrating, checks the experimental container and fixed worker launcher, validates embedded translations, then starts the configured adapters and queue workers.

```text
yoink-server
  -> yoink-http             diagnostics
  -> yoink-telegram         owner-private commands and native media delivery
       -> yoink-i18n        embedded EN/RU TOML catalogs
  -> yoink-downloads        admission, reauthorization, profiles and worker lifecycle
       -> yoink-identity   read-only legacy identity and grants
       -> ProfileStore     read-only legacy settings and cookie/proxy selection
       -> yoink-storage    durable queue, leases, quotas and delivery journal
       -> yoink-executor   fixed worker process and artifact validation
  -> Python worker          yt-dlp, gallery-dl and bounded ffmpeg processing
```

The available media commands are `/download <http(s) URL>`, `/job <UUID>`, and `/cancel <UUID>`. The outer Telegram gate admits only the configured owner in a private chat. The media service independently checks current identity and grants at admission, before execution, and before each delivery unit.

## Credentials and request snapshots

Admission loads supported stored user preferences and freezes opaque credential references. Personal cookies are separate from shared pool cookies, which require a current grant even when uploaded by the requesting actor. Proxy use follows the stored enable flag, preferring a personal proxy over the configured system fallback. Enabled-but-missing proxy configuration is rejected rather than silently using direct access.

The queue stores identifiers, ownership metadata and credential versions, not cookie content or proxy passwords. Idempotent replays retain the first admitted snapshot. Resolution rereads current credentials and access before execution and delivery. Changed, deleted or revoked references fail rather than selecting replacement credentials. Actual credentials enter the worker through its bounded stdin protocol.

Cache identity includes actor, bot, media parameters and selected credential versions. This separates credential-dependent results, but persisted cache keys alone do not implement Telegram file-ID cache reuse.

## Ownership boundaries and limitations

- Alembic owns existing `public` tables. Rust reads identity, settings and credentials without modifying them.
- SQLx migrations own only `yoink_runtime`, including operations, attempts, leases and delivery outcomes. Startup checks compatibility and never migrates.
- The fixed Python worker runs under container read-only/noexec controls and Landlock, with process-group termination, network policy and resource budgets. See the [media slice guide](media-slice.md).
- Worker engines support video/audio processing, playlists, clips and galleries. Transport options and settings are not yet at full Python parity. Interactive menus, group/topic routing, cache reuse, unsupported provider authentication and other product modules remain unfinished.
- This is neither a production cutover nor a performance claim. See [database ownership](../decisions/0001-database-ownership.md).
