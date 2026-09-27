# Architecture

The Rust workspace in [backend/Cargo.toml](../../backend/Cargo.toml) is a parallel implementation stage, not a replacement backend. The first opt-in media slice connects Telegram commands to identity authorization, PostgreSQL queueing, a container Python worker, and Telegram delivery. It remains experimental. Existing public/legacy tables are read-only, and there has been no production cutover.

## Runtime path

The server binds HTTP diagnostics first and reports readiness only after configured dependencies initialize. Database and Telegram adapters are optional. If downloads are enabled, both are required; startup verifies the existing Rust schema without modifying it, checks for the expected container marker and fixed worker paths, prepares Telegram, then wires the download service into the Telegram adapter and starts queue workers.

```text
yoink-server
  -> yoink-http            diagnostics
  -> yoink-telegram        optional polling and owner-private commands
  -> yoink-downloads       identity rechecks, PostgreSQL queue, worker lifecycle, delivery state
       -> yoink-identity  read-only public identity snapshot
       -> yoink-executor  fixed container Python process and artifact validation
       -> yoink-storage   SQLx PostgreSQL queue and schema check
       -> Telegram Bot API sendDocument
  -> Python worker         progressive HTTP(S) video only
```

The available media commands are `/download <http(s) URL>`, `/job <UUID>`, and `/cancel <UUID>`. The Telegram outer gate still admits commands only from the configured owner in private chat. Separately, the media service checks the requested owner's stored role and blocking status at admission, immediately before execution, and again before delivery.

## Ownership boundaries and limitations

- Existing tables in `public` remain owned by the Python service's Alembic chain. Rust reads selected identity rows and does not write them.
- Rust SQLx migrations own only `yoink_runtime`, including queue, attempts, leases, cancellation, quotas, and delivery outcomes. Normal server startup checks schema compatibility and never migrates.
- The Python process runs at the fixed image path with explicit time, socket, output, artifact, and workspace budgets. Worker process-group termination and reaping are lifecycle controls, not a security sandbox. Use container isolation and resource limits; see the [media slice guide](media-slice.md).
- The slice only handles one progressive HTTP(S) MP4 or WebM video artifact. It does not provide audio, playlists, HLS/DASH, multi-artifact jobs, clipping, subtitles, cookies, proxies, tdlight, or other plugins.
- The implementation is experimental and is not a production cutover or a performance claim.

See [database ownership](../decisions/0001-database-ownership.md) for the boundary between Alembic and SQLx.
