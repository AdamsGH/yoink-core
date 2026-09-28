# PostgreSQL import acceptance fixture

Run from `backend`:

```sh
just test-import
```

This builds both host Rust executables. `just verify-import` also checks the workspace, builds the pinned-compiler importer image, and runs a real TLS import through that image. The fixture requires Docker, host Node.js and OpenSSL, and the existing Alembic image. It never runs host Python.

The runner applies the actual Alembic chain inside the disposable fixture from `../legacy-fixture.mjs`. It creates a SELECT-only importer role, seeds all 41 application tables from live catalog metadata with stable IDs and representative values, and invokes the importer with its source URL only in `YOINK_IMPORT_SOURCE_URL`. Native checks use the fixture server's JSON SQL protocol and cast wide values to text before JSON decoding.

Assertions cover exact table row counts, independent source-to-native digests of every copied field, separate source-immutability digests, wide integer IDs and JSON numbers, SQL NULL versus JSON null, Unicode normalization, aliases/prompt checks, sequence high-water and empty-table allocation, destination privacy, schema/data rejection including RLS, destination/sidecar/symlink/parent safety, CLI and network policy, deterministic snapshot visibility, verified TLS, and SIGTERM cleanup. Process failures retain bounded stdout/stderr and a safe error category for diagnostics.
