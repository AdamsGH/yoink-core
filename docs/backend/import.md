# PostgreSQL data import

The operator-only `yoink-import` executable copies the reviewed Python application schema into a new native Turso database. It is not a PostgreSQL storage adapter for the server and does not switch the running deployment. Build and acceptance commands live in [`backend/justfile`](../../backend/justfile).

## Source and destination

Supply the source connection string explicitly in `YOINK_IMPORT_SOURCE_URL` through your existing credential delivery mechanism. The importer does not read `.env`, use `YOINK_RUST_DATABASE_PATH`, or discover a Compose database. Do not put the source password in command arguments, committed files or shell history. Environment variables remain readable to privileged process and Docker administrators.

Use a dedicated source principal with CONNECT on the database, USAGE on the public schema, SELECT on all imported tables and SELECT on their sequences. It needs no application writes, DDL privileges, or ownership. The client also starts an explicitly read-only repeatable-read transaction. Normal concurrent writes are not a substitute for stopping writers at the eventual cutover: the imported file represents one snapshot, not ongoing replication.

The default connection requires TLS with certificate and hostname verification. A private CA can be supplied with `--ca-cert`. Plaintext requires `--allow-loopback-plaintext` and an explicitly local endpoint. This exception is for numeric loopback or Unix sockets, not an arbitrary host on a private subnet. It must not be used to bypass TLS for a remote source.

The destination must not exist. Its parent directory must already be private (mode 0700), owned and writable by the importing operator. Existing database files, sidecars and unsafe paths are not overwritten. Imported data includes credentials and private content from the source: protect the resulting file and any interrupted staging directory accordingly.

From `backend/`, with `YOINK_IMPORT_SOURCE_URL` already supplied:

```sh
just build-import
target/debug/yoink-import --help
just import-postgres /absolute/private/path/imported.db
```

For a deliberately local source that has no TLS, use the explicit exception rather than weakening the default command:

```sh
target/debug/yoink-import --destination /absolute/private/path/imported.db --allow-loopback-plaintext
```

`just build-import-image` builds the opt-in `yoink/rust-importer:experimental` image with the pinned Rust compiler. This image contains neither Python nor media tools. The default backend image does not contain the importer executable. Container use needs the same private destination ownership and explicit source credentials. A read-only container root also needs a private writable temporary filesystem for both `TMPDIR` and `TURSO_TMPDIR`, separate from the destination directory.

## Conversion contract

The supported source is the actual Alembic schema recorded in [`legacy-postgres.json`](../../backend/schema/legacy-postgres.json), revision `0054_restore_stats_latest_view`. Unknown schema changes require review rather than silently dropping columns or tables. The importer takes shared table locks to prevent concurrent changes to the imported table definitions. Row-level security, extra partitioned tables, foreign tables and materialized views are rejected instead of producing a partial copy.

All 41 application tables are copied. Alembic bookkeeping is replaced by native migration history. PostgreSQL's derived `stats_messages.text_search` vector is not copied. Native V3 triggers queue the imported messages for the derived [Tantivy search index](stats-search.md). The approved new search contract has explicit tokenization differences from PostgreSQL. A successful import alone does not verify search or complete the Stats product port. An existing experimental `yoink_runtime` PostgreSQL schema is not silently discarded.

- User and group identifiers remain externally assigned, including wide and negative IDs. Internal serial identifiers and conservative sequence high-water marks are retained. PostgreSQL sequence counters are not MVCC data, so the observed sequence mark can include allocation after the row snapshot.
- Timestamps become signed UTC microseconds. Unrepresentable or infinite timestamps fail the import instead of becoming null or a rounded date.
- Booleans become checked 0/1 integers. Integer ranges, finite floating-point values, SQL NULL versus JSON null versus empty strings, Unicode text and JSON values must survive conversion without silent truncation.
- `inbox_categories.normalized_name` is copied from PostgreSQL. Recomputing it with native `lower()` would change Unicode normalization behavior.
- Native uniqueness is stricter for default thread policies with a null thread ID and for Stats chat/message pairs. Conflicting source rows must be resolved explicitly in the source before retrying. The importer does not choose a winner or merge records.

The importer streams rows and verifies typed per-table SHA-256 digests against the staged database. `--max-row-bytes` limits accepted encoded rows, not process RSS: the PostgreSQL driver receives the current row before that check. Use container memory limits when the source can contain unusually large rows.

## Publication and recovery

Import runs in a private sibling staging directory. The target name is reserved against a cooperating runtime owner. The native schema is migrated explicitly, rows are copied, relationships and imported values are checked, and the standalone database is checkpointed and closed before publication. Atomic no-replace publication must not replace an existing destination that appeared during the import.

An unsuccessful pre-publication import leaves no database at the target name. Abrupt process death can leave private staging or lock artifacts. Do not remove them based only on their age: first confirm that the importing process and any database owner have stopped, then inspect and remove only that failed run's artifacts. Never copy an active staging database or delete sidecars from a live owner.

A failure reported after publication is different from a failure before it. If the final directory sync cannot be confirmed, retain the complete published file and resolve that durability uncertainty rather than rerunning over it. No rollback operation should delete an already published database automatically.

## Cutover remains explicit

A verified import is not permission to start the Rust service against production bots or credentials. Before a later cutover, finish product and search compatibility, verify native backup/restore, stop source writers and pollers, take a fresh import, validate it, and start exactly one native owner. Keep the PostgreSQL deployment and its data intact until the rollback plan has been exercised.

Before new native writes occur, abandoning the candidate means stopping its owner and retaining the file for inspection while continuing the untouched PostgreSQL deployment. After native writes occur, restarting the old service loses those new changes unless a separately verified reverse-migration procedure exists. No such automatic reverse import is provided here.

## Acceptance

Run `just verify-import` for workspace checks, the real-Alembic fixture, source read-only/snapshot tests, native content and safety assertions, and the pinned-compiler image build and import. The fixture independently compares every copied column in all 41 tables, exercises a SELECT-only source role, and verifies trusted-CA success and certificate/hostname rejection. It also imports through the container CLI with a read-only root, private tmpfs and the source fixture's isolated network namespace. Fixtures use synthetic rows and disposable PostgreSQL containers. Python is executed only inside the existing migration container. This command must not discover or connect to the running deployment.

The fixture covers supported shapes and failure cases, not arbitrary PostgreSQL extensions, schema drift, sudden power loss, cross-version engine recovery, or an already-running native destination. The [native backup/restore utility](backup-restore.md) now extends the synthetic import fixture through an independent restored-content comparison. Production cutover remains a separate task.
