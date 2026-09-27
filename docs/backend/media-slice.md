# Experimental media slice

This page covers an isolated evaluation of the first implemented media path. It is not a production deployment guide or a production cutover plan. For exact setting defaults, ranges, and cross-field checks, see [configuration](configuration.md).

## What is implemented

With downloads explicitly enabled in the experimental container, the configured Telegram bot accepts `/download <http(s) URL>`, `/job <UUID>`, and `/cancel <UUID>`. The Telegram adapter's outer gate remains only the configured owner in a private chat. The service also checks the legacy identity store and requires an unblocked private-scope user-role identity at admission, before claim execution, and before delivery. This does not open Telegram commands to all users.

The Rust server admits work to a durable PostgreSQL queue, applies attempt, delay, global and per-user pending limits, claims leased work, and starts the fixed isolated Python worker. Protocol v2 returns a bounded batch of validated artifacts with typed progress and failures. The worker implements yt-dlp video/audio processing, playlists and clips, plus gallery-dl image batches and ZIP output. Delivery uses Telegram video/photo/audio methods where supported and documents otherwise, with compatible batches grouped into albums of at most ten items. Job lookup and cancellation are actor-scoped. These engine capabilities do not yet expose the complete legacy command and settings surface.

## Isolated operator run

Build only the experimental image and supply synthetic fixture credentials and an isolated database. Never reuse the Python service's live bot credential. Competing long-polling consumers can compete for updates, and admission idempotency includes bot ID, chat ID, and Telegram message ID.

This concrete container invocation uses example-only values, host networking for an isolated local PostgreSQL and fixture Bot API, and the recommended container boundary. Set `YOINK_RUST_TELEGRAM_TOKEN` in the shell to a disposable fixture credential before running; no live credential is shown here:

```sh
cd backend
just build-media
docker run --rm --network host --read-only \
  --cap-drop ALL --security-opt no-new-privileges --pids-limit 128 --memory 512m \
  --tmpfs /tmp/yoink-work:rw,noexec,nosuid,size=256m,uid=10001,gid=10001,mode=0700 \
  -e YOINK_RUST_DOWNLOADS_ENABLED=true \
  -e YOINK_RUST_TELEGRAM_ENABLED=true \
  -e YOINK_RUST_TELEGRAM_TOKEN \
  -e YOINK_RUST_OWNER_ID=42 \
  -e YOINK_RUST_BOT_API_URL=http://127.0.0.1:8081 \
  -e YOINK_RUST_DATABASE_URL='postgresql://fixture-user:fixture-password@127.0.0.1:5432/isolated' \
  yoink/rust-backend:experimental
```

The example's 256 MiB tmpfs covers the default 100 MiB workspace budget times the default concurrency of two, plus margin. Recalculate it if either setting changes. The database must be explicitly migrated before startup; the fixture Bot API, database, source endpoint, and network access must be arranged separately. Do not run against production credentials or data. The actual end-to-end fixture is `cd backend && just verify-media`; its recipe and current verification status are described in [development](development.md).

## Container boundary and hardening

The worker applies destination-address checks for requests and redirects, blocking private, loopback, link-local, multicast, reserved, and IPv4-mapped private targets by default. yt-dlp plugins, ambient configuration and external downloaders are disabled. Protocol credentials arrive only on stdin, never in process arguments or diagnostic fields. Proxy endpoints require a separate explicit origin allowlist. Destination checks remain active when using a proxy, including CONNECT handling. The service resolves authorized, versioned cookie/proxy references against current legacy settings before execution and each delivery unit. A changed proxy enablement policy invalidates frozen direct routes instead of allowing direct fallback or selecting a replacement proxy. ffprobe and ffmpeg use protocol/demuxer allowlists and bounded processing deadlines. Per-artifact, aggregate workspace, item count, output, socket and process budgets are enforced by the Rust executor and worker.

The launcher adds Landlock filesystem and process-access restrictions, with an owner-only workspace and no inherited application secrets. Startup fails when required confinement is unavailable. Process-group termination and reaping handle lifecycle management but do not replace confinement. Keep the container read-only, its workspace non-executable, and sensitive mounts absent:

```text
--read-only
--cap-drop ALL
--security-opt no-new-privileges
--pids-limit 128
--memory 512m
--tmpfs /tmp/yoink-work:rw,noexec,nosuid,size=<budget>,uid=10001,gid=10001,mode=0700
```

Set tmpfs size to cover at least `DOWNLOAD_WORKSPACE_MAX_BYTES * DOWNLOAD_CONCURRENCY` plus margin for process and filesystem overhead. The image runs as UID/GID 10001 and the workspace needs to be writable by that identity. The private-origin allowlist is only for a trusted operator's isolated local fixture, not production destinations. Network egress controls and other container boundaries remain the operator's responsibility.

## Delivery ambiguity, retries, and cancellation

Before upload, the queue records the batch delivery plan and journals each delivery unit before its request. Valid Telegram message IDs confirm a unit. All units confirmed records `succeeded`. A known rejection before any confirmation records `failed`, while confirmed earlier units followed by a known failure record `partial`. Unrecognized API errors (including server-side errors), a dropped/ambiguous response, timeout, crash, heartbeat loss, or stale in-flight delivery lease produce terminal `delivery_unknown`, with no automatic resend. Recorded receipts preserve already confirmed units across recovery. Exactly-once Telegram delivery is not claimed.

If `/job <UUID>` reports `delivery_unknown`, check the chat before submitting another request. A new request can duplicate a delivery that Telegram accepted while its response was lost. Cancellation before upload stops and reaps the worker. Once upload starts, delivery outcome takes precedence over cancellation to avoid an uncertain cancel-and-retry send. Queue retries apply to retryable worker failures and definitively unsent Telegram rate limits while attempts remain. A rate-limited first unit waits at least both the configured delay and Telegram's `retry_after`. A batch with a confirmed prefix is not restarted. Permission revocation stops unsent work promptly without marking it as user cancellation. Shutdown can requeue a batch only before any send and within the attempt budget.

## Not implemented

Full downloader parity is still in progress. Supported settings and authorized cookie/proxy snapshots are wired into the service, but legacy interactive commands/callbacks, remaining settings and subtitle compatibility, file-ID cache reuse, group/topic access, tdlight and HTTP product routes are unfinished. Other product plugins are not implemented. This is not a production cutover and makes no performance claim.
