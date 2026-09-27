# Development and database verification

## Prerequisites

Use the Rust toolchain required by [backend/Cargo.toml](../../backend/Cargo.toml) (declared MSRV 1.88), Node.js 22+, `just`, Docker, and the required local images. The experimental media image is built with Rust 1.88 and Python 3.12, and pins yt-dlp 2026.8.19 and gallery-dl 1.32.13. The media launcher requires Linux Landlock ABI 6 or newer. Python commands run inside Docker only. Do not run host `uv`, `pip`, `python`, or `pytest`.

## Commands

Run Rust workspace commands from the repository root:

| Command | Purpose |
| --- | --- |
| `cd backend && just format` | Apply Rust formatting. |
| `cd backend && just fmt` | Check Rust formatting. |
| `cd backend && just lint` | Clippy all workspace targets with warnings denied. |
| `cd backend && just test` | Workspace tests. |
| `cd backend && just verify` | Formatting, lint, unit tests, non-database smoke checks, script syntax checks, and documentation link checks. It does not require Docker. |
| `cd backend && just test-db` | Disposable PostgreSQL/Alembic integration fixture and database process checks. |
| `cd backend && just verify-db` | `verify` plus the database fixture. Uses disposable PostgreSQL and the legacy image for Alembic. |
| `cd backend && just build-media` | Builds the opt-in experimental image; does not alter the existing Compose deployment. |
| `cd backend && just verify-media` | Builds the image, runs the Python engine/security suites in a network-disabled container, checks the default launcher under deployment hardening, then runs the real media fixture with disposable PostgreSQL and a mock Bot API. |

The media fixture verifies real worker download and native Telegram multipart upload under read-only/noexec and Landlock confinement. It checks bounded MP4 framing and unchanged media payload across remuxing, replay idempotency, actor isolation, cancellation and cleanup, three persisted HTTP 503 attempts with retry delay, terminal API rejection, a durable Telegram 429 retry respecting `retry_after`, API 500 and dropped-response ambiguity, crash recovery without resend, admission quotas, access revocation before execution and delivery, and secret redaction. Telegram is a local fixture API, not a live service or a platform-compatibility test.

`just build yoink` builds the legacy `yoink/yoink:latest` image used for the Alembic fixture; it does not start services. All Python work remains inside containers.

## What the database fixture does

`backend/scripts/test-db.mjs` uses only already-present images (`--pull never`). It creates a randomly named PostgreSQL container with a generated password, PostgreSQL data on tmpfs, and a random host port bound only to loopback. It does not read `.env` or accept a caller database URL. It applies the real Alembic chain from a read-only bind mount using `yoink/yoink:latest`, then checks that schema inspection rejects a database before explicit Rust migration.

It serially runs the ignored SQLx storage and identity database test suites, runs `migrate` twice and `check-schema`, starts the diagnostic server, and confirms it exits unsuccessfully when the database is stopped. Cleanup removes only the named fixture container and its volumes. Do not run concurrent database fixtures against the same instance.

## Media fixture scope and schema lifecycle

`backend/scripts/test-media.mjs` uses a disposable PostgreSQL database, legacy Alembic migrations and fixtures, a fixture Bot API, a local progressive-video HTTP source, and the real experimental worker container. It does not use a live bot, production credentials, or the application database. See the [media slice guide](media-slice.md) for limits and isolation guidance.

The public schema is migrated by Alembic. SQLx migration history and runtime operations belong to `yoink_runtime`. Run database commands only against a deliberately isolated database URL:

```sh
cd backend
YOINK_RUST_DATABASE_URL='postgresql://user:password@127.0.0.1:5432/isolated' just migrate
YOINK_RUST_DATABASE_URL='postgresql://user:password@127.0.0.1:5432/isolated' just check-schema
```

`migrate` explicitly creates the Rust schema and applies embedded SQLx migrations. `check-schema` and normal startup are read-only: they reject absent, dirty, unexpected, or checksum-mismatched histories rather than repairing them. There is no automatic rollback or drop. Never point these commands or fixtures at a production database.
