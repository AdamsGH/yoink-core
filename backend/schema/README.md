# Rust-owned legacy schema

The PostgreSQL catalog snapshot is generated from the actual Alembic chain in a disposable fixture, not from production rows. It defines the supported import source shape. Native Turso DDL is reviewed and versioned separately from PostgreSQL migrations.

PostgreSQL remains read-only during import. Unknown source columns, tables or types must fail validation rather than be silently discarded. Runtime operation tables use the `runtime_` prefix and have a separate source mapping.
