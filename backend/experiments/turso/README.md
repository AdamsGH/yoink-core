# Isolated native Turso evaluation

## Result

Native `turso = 0.7.2` passes the tested queue, transaction and recovery contracts. It is a viable candidate for the next storage implementation slice, not approval to replace the running PostgreSQL deployment. The experiment stays outside the server workspace and has its own committed Cargo.lock. It uses neither libSQL nor Turso Cloud, and enables no experimental features.

All test data is synthetic and lives in temporary directories. No fixture reads deployment settings or a production database. One Rust process owns one `Database` handle with independent connections. A crash-test child owns a different disposable file and exits before the parent reopens it.

## Verified contracts

| Contract | Evidence |
| --- | --- |
| Idempotent admission | Repeated and four-way concurrent actor/request keys produce one job. Duplicate admission preserves the first payload. |
| Atomic multi-statement failure | A constraint error followed by rollback leaves no actor or request from the failed transaction. |
| Contended claims | Eight connections on four Tokio worker threads claim 120 jobs with no duplicate or missing ID. A barrier starts the contenders together. |
| Busy handling | Retry only typed `Busy` and `BusySnapshot`, with bounded attempts and async backoff. Other errors fail the test. This does not establish that both variants occurred in the observed run. |
| Lease fencing | Wrong generation, wrong token and expiry reject completion. An expired lease cannot start sending. Reclaim increments the generation on the next claim. |
| Ambiguous delivery | A persisted `sending` job becomes `delivery_unknown` after reopen and is not claimable. |
| Constraints | Actual writes test primary-key, composite-key, job-unique, check and foreign-key enforcement. Each CHECK test avoids accidentally failing only on an existing UNIQUE key. |
| Cancellation | A handshake proves a transaction wrote before its future is dropped. Strict insertion of that same primary key succeeds afterward, proving rollback before connection reuse. |
| Reopen and process crash | Committed rows survive normal reopen. The parent observes a child die from SIGKILL after it commits queued/sending jobs and opens another uncommitted transaction. Recovery retains the committed work, discards the uncommitted actor and does not replay the ambiguous send. |
| Backup and restore | `VACUUM INTO` produces a nonempty file. Reopening it preserves payload, schema and enforced constraints, and excludes a write made after the snapshot. Missing-file restore fails instead of creating an empty database. |

Every writing connection enables foreign keys, FULL synchronous mode and a 250 ms busy timeout. Multi-statement writes use `BEGIN IMMEDIATE`. Query rows are drained before transaction completion. No network effect occurs inside a transaction.

## Commands and results

Run from `backend/`:

```sh
just verify-turso
just verify-turso-container
```

`verify-turso` checks formatting, Clippy with warnings denied, and tests. The observed host compiler was Rust 1.97.1. `verify-turso-container` compiles and runs the tests with the pinned server compiler and disables network access during compilation and test execution. Dependency fetching is a separate build step. Both suites have a 90-second deadline after compilation.

The original evaluation passed 13 parent tests. Recorded follow-up checks passed **14 tests, 0 failed, 1 ignored** after adding the 48-case rowid-FK regression. The ignored item is the crash-child entrypoint, invoked explicitly by the passing SIGKILL parent test. It is not missing crash coverage. The original PostgreSQL-backed server baseline passed 135 tests with 16 integration tests ignored. Those historical counts do not describe the current native runtime.

The package declares Rust 1.88 and its lockfile selects Roaring 0.11.3. Unconstrained resolution initially selected Roaring 0.11.5, which needs Rust 1.90. The original evaluation did not change the server compiler or dependency lockfile.

## Foreign-key RESTRICT regression probe

`tests/evaluation.rs::rowid_reference_delete_requires_explicit_child_index` covers 48 cases: `RESTRICT`/`NO ACTION`, inline/table-level FK, fresh/post-query-DDL/reopened connection, and four child-column layouts. All 12 unindexed `INTEGER PRIMARY KEY` child references allow an invalid parent delete. All 36 indexed-rowid, `INT PRIMARY KEY`, and ordinary-column references reject it. Each case checks enabled/declarative FKs and rollback preservation. This is a rowid-alias lookup defect, not a schema-cache timing problem.

The pinned engine's `translate/fkeys.rs::table_scan_match_any` emits `Column` for the child key, including rowid aliases, whose record-column value is NULL. An explicit child index selects a different, correct lookup path. Native migration V4 adds indexes for all six affected application FK keys. Storage regression `version_three_upgrade_preserves_rows_and_protects_all_rowid_foreign_keys` checks their real `ON UPDATE NO ACTION` restrictions, existing rows, and unchanged delete cascades.

Run the focused probe from `backend/`: `cargo test --manifest-path experiments/turso/Cargo.toml --target-dir target/turso-probe --locked -- --exact rowid_reference_delete_requires_explicit_child_index --nocapture`. The test deliberately records the unsupported unindexed layout under the pinned version. Do not introduce a new rowid-alias FK without its explicit child index.

## Native backup limitations

Turso 0.7.2 rejects a bind parameter in `VACUUM INTO` with `VACUUM INTO requires a string literal path`. Its path extractor also strips surrounding quotes without decoding SQL-doubled apostrophes. A destination containing an apostrophe therefore produced a differently named file in the initial test.

The fixture's `backup_into` substitutes a restricted literal path from a SQL file. It rejects quote and NUL characters before executing SQL and checks that the requested nonempty file exists afterward. Spaces are covered. This restriction does not fix native arbitrary-path handling, and the fixture helper is not a production backup API. The separate [operator implementation](../../../docs/backend/backup-restore.md) uses an owned directory descriptor and a fixed `/proc/self/fd/<fd>/snapshot.db` literal, supporting quoted destination paths without interpolating user-controlled SQL.

## Limits and next decision

These tests do not simulate sudden power loss, torn writes, disk-full conditions, cancellation during a pending OS write, long-running migrations or cross-version recovery. No throughput comparison with PostgreSQL or SQLite was performed. The cancellation test drops the transaction after a completed statement, not halfway through native I/O.

The proposed ownership is one backend process. Multiple processes sharing the live file, simultaneous SQLite/Turso access, experimental concurrent transactions and cloud synchronization remain out of scope. Media workers must not open the database file.

Stats full-text search is still a migration gate because Turso 0.7.2 marks FTS experimental. SQLite file-format compatibility does not imply complete SQL compatibility or a tested downgrade route. The next implementation must port actual repository queries, decide search semantics and verify data conversion and rollback before cutover.

See [decision 0002](../../../docs/decisions/0002-turso-evaluation.md). [Decision 0001](../../../docs/decisions/0001-database-ownership.md) still governs the implemented PostgreSQL runtime.
