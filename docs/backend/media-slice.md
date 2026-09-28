# Experimental media slice

This page describes the implemented Rust media architecture and its isolated verification route. It is not a production deployment guide or cutover plan. For detailed setting limits, see [configuration](configuration.md).

## Runtime and guarantees

With downloads enabled, the configured Telegram bot accepts `/download <http(s) URL>`, `/video URL`, `/audio [range] URL`, `/image URL`, `/playlist [range] URL`, inline `/cut URL START END`, standalone `/subs URL`, `/job <UUID>`, and `/cancel <UUID>`. See the [downloader compatibility matrix](downloads-parity.md) for exact scope, range/clip semantics and remaining workflows. Private commands remain owner-only. A separate group opt-in enables known, authorized users in registered groups/topics without changing their global role. The media use case requires current authorization for the original destination at admission, execution, delivery and progress. See [access control](access-control.md) for exact limits.

Operations, leases, quotas, cache and delivery journals persist in the native Turso 0.7.2 database. One process owns the file through an advisory sidecar lock, and all calls use one serialized connection. No PostgreSQL, SQLx, or PostgreSQL privilege model participates in this runtime. The trusted server process owns access to all database contents, so filesystem ownership and private directory permissions protect credentials at rest. Access snapshots must not return credential values.

The service freezes credential references and versions, never credential contents. It rechecks grants and references before work and each delivery unit. Queue claims use expiring leases and fencing tokens; stale workers cannot transition durable state. Retries are bounded. Cancellation stops and reaps a running worker. Delivery state is journaled before sending, confirmed units survive recovery, and uncertain outcomes are terminal rather than automatically resent. Exactly-once external delivery is not promised.

The fixed Python worker performs media processing in a constrained container. The documented engine capabilities include video/audio processing, playlists, clips, galleries and standalone SRT/VTT subtitles, but not full legacy command or settings parity. The native media acceptance fixture covers real worker execution, persistent delivery state and recovery, access changes, retry/cancellation/quota behavior, file-ID cache recovery/expiry and credential redaction. Video, audio and inline-cut commands are checked through uploaded bytes with ffprobe. Additional batch scenarios verify generic HTML playlists through ordered albums, cached album reuse and durable partial delivery, plus a gallery-dl direct image through native photo delivery. Group scenarios verify original topic/reply routing, cache and album destinations, revocation during download and uncertain-receipt recovery. Separate gallery scenarios use the unchanged Imgbox extractor over synthetic HTTPS pages to verify multi-image albums and ZIP delivery. Subtitle scenarios extract local HTML tracks, compare uploaded document bytes and test cache isolation, restart and guarded failure paths without downloading video. These fixtures do not establish live-provider compatibility.

## Isolated container layout

The default production runtime image does not contain the fixture controller. `backend/Dockerfile` has a separate opt-in `media-fixture` target that includes a test-only token-protected controller. The fixture controller only binds to loopback, is bounded, and shares the runtime's native DB path so tests can inspect and seed the same database. Never expose this image/controller as an operator API.

The runtime image runs as UID/GID 10001. Mount a private, writable persistent state volume owned by UID 10001 at `/var/lib/yoink`; use an isolated fixture path such as `/var/lib/yoink/media-fixture.db`. The parent must already exist and be mode `0700`. A separate work directory is still required and must be writable by UID 10001. Use synthetic fixture bot credentials and the fixture's loopback-only controller settings. Do not use a host PostgreSQL URL, real credentials, or production data.

Run the supported sequence from `backend`:

```sh
just verify-media
```

This builds both the normal runtime image and the separate fixture image, then runs worker/security checks, validates the default hardened worker launcher, and invokes the full media fixture using its own named native volume. Do not share that volume with an operator database. Worker unit/security tests alone do not prove the native database queries or complete media workflow work.

## Worker boundary

The worker applies destination-address checks to requests and redirects, blocking private and otherwise prohibited destinations by default. Plugins, ambient configuration, and external downloaders are disabled. Credentials travel only through the bounded stdin protocol, not process arguments or diagnostic fields. Proxy endpoints require a separate explicit origin allowlist, and destination checks remain active through proxies. ffprobe and ffmpeg use protocol/demuxer allowlists and bounded deadlines. Artifact, aggregate workspace, item, output, socket, and process limits are enforced.

The launcher adds Landlock filesystem and process restrictions, uses an owner-only workspace, and does not inherit application secrets. Startup fails when required confinement is unavailable. Keep the container read-only, its workspace non-executable, and sensitive mounts absent:

```text
--read-only
--cap-drop ALL
--security-opt no-new-privileges
--pids-limit 128
--memory 512m
--tmpfs /tmp/yoink-work:rw,noexec,nosuid,size=<worker-budget>,uid=10001,gid=10001,mode=0700
--tmpfs /tmp/yoink-db:rw,noexec,nosuid,nodev,size=<database-scratch-budget>,uid=10001,gid=10001,mode=0700
--env TMPDIR=/tmp/yoink-db
--env TURSO_TMPDIR=/tmp/yoink-db
```

Keep the database scratch tmpfs separate from the Landlock-accessible worker workspace. Turso creates temporary files during query execution, even when the persistent database volume is writable. The fixture uses a 64 MiB scratch budget; larger datasets need a separately chosen and tested bound. Removing the container removes this scratch state without affecting the persistent database.

Size temporary workspace storage for at least `DOWNLOAD_WORKSPACE_MAX_BYTES * DOWNLOAD_CONCURRENCY` plus process overhead. The private-origin exception is only for a trusted isolated fixture, not production destinations. Network egress controls remain the operator's responsibility.

## Current limitations and pending gates

Legacy command/settings parity, registration and private-delivery redirects, unsupported provider authentication, and other product modules remain unfinished. The [PostgreSQL importer](import.md) is verified on isolated fixtures, not production data. The separate [Stats search component](stats-search.md) has an approved Tantivy contract. [Native backup/restore](backup-restore.md) is verified on isolated data. Stats product integration and production cutover remain pending. A live database file plus its WAL must not be copied as a backup.
