# 0002: Historical Turso evaluation

Status: historical evaluation. Superseded for the Rust runtime by [decision 0003](0003-native-turso-runtime.md), 2026-09-27.

## Scope and finding

This record documents the earlier isolated evaluation of the native Rust `turso` crate, pinned to 0.7.2, rather than libsql, Turso Cloud, or a PostgreSQL compatibility frontend. That evaluation considered transactional admission, leases, fencing, delivery uncertainty, constraints, cancellation, and reopen behavior. It was not an approval to import production data or cut over the deployed Python service.

The resulting Rust server now uses native Turso, but that runtime is described by [decision 0003](0003-native-turso-runtime.md). The historical proposal's multi-connection ownership model, SQLx/PostgreSQL migrations, and shared database are not current runtime facts. The native runtime instead has one owning server process, one serialized connection, an advisory sidecar lock, and explicit file-backed migrations.

## Historical verification boundary

Passing isolated storage contracts is not evidence of PostgreSQL schema import, Stats full-text parity, backup/restore readiness, or production cutover. Those require independent implementation, validation, and approval. The current deployment uses native Turso. This historical evaluation did not establish production migration safety.

For current database configuration and operator procedures, see [backend configuration](../../operations/configuration.md), [persistence](../persistence.md), and [development](../../development/verification.md).
