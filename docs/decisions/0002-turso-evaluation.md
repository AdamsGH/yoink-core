# 0002: Evaluate embedded Turso before changing database ownership

Status: isolated evaluation passed, 2026-09-27. Proceed with Turso as a candidate for the next storage implementation slice. No deployment cutover authorized.

## Context

A read-only inspection of the running PostgreSQL database found 14 MB total database size, about 5.5 MB of user relations including indexes, 8,326 messages, 3,143 reactions, 165 downloads and 38 users across about 50 days of recorded activity. These are storage and historical row counts, not a measured peak transaction rate. Volume alone does not justify a separate database server.

Yoink still needs atomic operation admission, unique ownership of claimed work, expiring leases, stale-worker fencing, durable delivery state and reliable recovery. A small database does not relax those requirements. Existing Stats queries also use PostgreSQL full-text search.

The user selected Turso for an experiment rather than DuckDB. DuckDB remains an analytical engine, not the proposed primary transactional store.

## Evaluation boundary

Use the native Rust `turso` crate pinned to `=0.7.2`, not libSQL, Turso Cloud or a PostgreSQL compatibility frontend. Disable default optional features and do not enable experimental features. Keep ordinary transactions rather than introducing `BEGIN CONCURRENT` without a demonstrated concurrency need.

The proposed ownership model is one Rust backend process opening one database handle and creating independent connections for its tasks. Media workers do not open the database file. No simultaneous SQLite/Turso access or multi-process WAL coordination is part of this proposal.

`backend/experiments/turso/` is a standalone Cargo workspace with its own committed lockfile. The server has no dependency on it. Fixtures create synthetic data only in temporary directories and never read deployment configuration or accept production database URLs. The separate Docker check uses the existing server compiler, Rust 1.88. Declaring that MSRV selects compatible dependencies, including Roaring 0.11.3 rather than 0.11.5, which requires Rust 1.90.

## Acceptance

- Unique admission and atomic multi-statement changes, including rollback on failure.
- Contending independent connections cannot claim the same operation twice.
- Lease generation, token and expiry reject stale transitions.
- Persisted ambiguous delivery becomes `delivery_unknown`, never automatically queued again.
- Primary-key, unique, check and foreign-key constraints are actually enforced.
- Transaction cancellation rolls back before connection reuse.
- Committed data survives reopen and an explicitly observed child-process `SIGKILL`, while uncommitted changes do not.
- A consistent backup can be reopened and checked independently of the source.
- The pinned compiler builds and runs the fixture without network access during execution.

The fixture is a focused evaluation of these primitives, not a second implementation of the complete production schema or a storage abstraction framework. Results and remaining limits belong in [the experiment report](../../backend/experiments/turso/README.md).

## Migration gates

Successful fixture tests do not authorize replacing the running database. A later implementation must port the actual repository queries, define schema and migration ownership, preserve IDs and relationships, verify data conversion, and prove backup/restore and rollback on representative data before a separately approved cutover. Do not split identity authority between live PostgreSQL and a writable Turso copy without an explicit consistency design.

Stats full-text search needs a separate decision. Turso 0.7.2 lists FTS as experimental. Replacing the current search with `LIKE` would change behavior and is not an accepted fallback. SQL compatibility is incomplete, so the SQLite-compatible file format is not proof of a drop-in driver or rollback path.

[Decision 0001](0001-database-ownership.md) remains authoritative for the currently implemented PostgreSQL runtime and the live Python deployment. This experiment does not supersede it.

## References

- [Pinned Turso release](https://github.com/tursodatabase/turso/tree/v0.7.2)
- [Pinned SQL and file compatibility](https://github.com/tursodatabase/turso/blob/v0.7.2/COMPAT.md)
- [Pinned Rust compiler](https://github.com/tursodatabase/turso/blob/v0.7.2/rust-toolchain.toml)
- [DuckDB concurrency model](https://duckdb.org/docs/current/connect/concurrency)
