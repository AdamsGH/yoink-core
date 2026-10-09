# Rust-owned legacy schema

The PostgreSQL catalog snapshot is generated from the actual Alembic chain in a disposable fixture, not from production rows. It defines the supported import source shape. Native Turso DDL is reviewed and versioned separately from PostgreSQL migrations.

PostgreSQL remains read-only during import. Unknown source columns, tables or types must fail validation rather than be silently discarded. Runtime operation tables use the `runtime_` prefix and have a separate source mapping.

## Revision 0054 import contracts

The importer accepts the original 42-table catalog, that catalog plus the
explicit `music-resolve-log.json` table, or the exact reviewed live profile.
The live schema-only regression fixture is
`crates/yoink-import/fixtures/live-catalog.json`. All profiles retain revision
`0054_restore_stats_latest_view`.

Comparison uses named tables, columns and constraints. Physical column ordinal,
table/constraint enumeration and index OID ordering do not affect copying by
named projections. Enum label order, types, nullability, generated expressions,
sequence names, defaults and constraint definitions remain checked. SQL catalog
queries now order indexes by definition rather than transient OID.

`legacy-live-profile.json` is an explicit patch allowlist, not a blanket
schema-equivalence claim. Each removal/replacement records its expected baseline
value. It contains exactly 63 observed missing server defaults, five reviewed
index sets, two absent constraints, one equivalent primary-key rename and the
absent derived Stats FTS trigger. Legacy ORM client defaults explain the missing
server defaults; import supplies every copied value explicitly. Unlisted default
changes, unknown tables/columns/constraints and altered FK actions still fail.
Mix-and-match profile combinations are not implicitly accepted.

The live profile lacks these safety constraints:

- `cookies_personal_unique`: unique `(user_id, domain)` when `is_pool=false`.
- `uq_insight_cache_key`: unique `(content_key, lang, command)`.
- `ck_insight_tldr_alias_shape`: aliases and prompt, or target_alias, or domains.

Read-only source preflights check all three in the same snapshot, before opening
or migrating the staged native DB. Failure identifies the constraint and key
fields without printing rows. Native constraints remain enforced; incompatible
data is rejected, never deduplicated or repaired. Full row digests and FK
verification still gate publication.

Absent source nonunique indexes only affect source query performance. Native
indexes are retained. The missing `stats_messages` FTS trigger/index is derived
state: raw text/caption and all other business columns are preserved, while
native migration 0003 triggers rebuild search state from imported messages.
The `stats_chat_admins` PK name changes from `pk_stats_chat_admins` to
`stats_chat_admins_pkey`; its ordered key fields are identical.

Catalog rejection reports the closest reviewed profile's differing field paths,
bounded and escaped, never source expressions or row values. Existing native
migration checksums are unchanged.

## Music persistence API

Native migration 0025 adds `music_resolve_log` with all seven legacy columns,
int4 bounds, character limits, the user FK with delete cascade and the
`(user_id, created_at)` index. The legacy model has client-side defaults for
`platforms_found` and `created_at`, so callers supply both explicitly.
Import includes this table in named copy, digest/FK verification and allocator
preservation, including a source sequence high-water mark above existing rows.

The Music lane uses `yoink_storage::NewMusicResolveLog`:
`user_id: i64`, `source_platform: String`, `artist/title: Option<String>`,
`platforms_found: i32`, `created_at: DateTime<Utc>`.
`Store::insert_music_resolve_log(&record) -> Result<i64, StoreError>` returns
the new ID. The user must exist. This is analytics persistence, not download
admission or message delivery.

`Store::music_resolve_logs(user_id, after_id, limit)` returns
`Vec<MusicResolveLog { id, resolve }>` ordered by ID. Use `None` for the
first page and a limit in 1..=1000. Callers enforce authorization before exposing
analytics. Times use the existing signed UTC microsecond codec; SQL NULL remains
distinct from empty artist/title. No HTTP, Telegram or server composition changes
are part of this schema/import contract.
