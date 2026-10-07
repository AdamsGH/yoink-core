# YouTube sidecar packaging and verification

This standalone topology does not replace production compose. Native execution remains opt-in through `YOINK_RUST_YOUTUBE_AUTH_ADAPTER_ENABLED`, which defaults off. See `youtube-auth-integration.md` for credential, admission and origin contracts.

## Pinned inputs

- Gateway and youtubei use Node 22.16.0 bookworm-slim, OCI index `sha256:048ed02c5fd52e86fda6fbd2f6a76cf0d4492fd6c6fee9e2c463ed5108da0e34`.
- youtubei uses youtubei.js 17.0.1 and Express 4.21.2. The npm lockfile pins transitive versions and integrity hashes. Installation uses `npm ci --ignore-scripts`.
- ffmpeg and apt dependencies use the signed Debian bookworm and security snapshot `20250611T000000Z`. Apt verifies Release signatures and artifact hashes. HTTP transport avoids a dependency on preinstalled CA roots.
- bgutil uses the upstream 2.0.0 artifact, OCI index `sha256:ed86b6fdd5e430ddd7c8ce1adb55e1ab54db7c7dbc1bcbf3a82454a85b971164`. It is not rebuilt from the youtubei lockfile.

These pins make fixture inputs reproducible. They do not establish current security approval. Review image provenance, licenses, vulnerabilities and target-platform support before deployment. Update pins through a reviewed change.

## Cookie lifecycle ownership

The backend server owns cookie polling, cleanup and durable result retention. Sidecars do not duplicate this lifecycle. Backend defaults are:

```dotenv
YOINK_RUST_COOKIE_OAUTH_POLL_BUDGET_SECS=120
YOINK_RUST_COOKIE_OAUTH_RESULT_RETENTION_SECS=1800
YOINK_RUST_COOKIE_OAUTH_MAX_INFLIGHT=128
YOINK_RUST_COOKIE_OAUTH_CLEANUP_INTERVAL_SECS=60
YOINK_RUST_SHUTDOWN_TIMEOUT_SECS=135
```

The effective poll-budget range is 61..=294 seconds. Shutdown must exceed the poll budget by more than five seconds, within the 300-second shutdown limit. A server container's stop grace must exceed its configured shutdown timeout. Use at least 150 seconds for these defaults.

The standalone worker is a one-shot launcher, not a backend server. Its environment does not carry these server settings. The runtime regression verifies that accepted polls survive HTTP cancellation and drain before the database closes.

## Isolation and bounds

`backend/docker-compose.youtube-auth.yaml` publishes no ports. Only the gateway, youtubei and bgutil join the internal service network. Only youtubei and bgutil join provider egress. The deterministic override removes provider egress from both and substitutes synthetic upstreams.

Operator-configured upstream origins are `http://youtubei:9173` and `http://bgutil:4416`. Media requests cannot change them. The worker has no network and mounts only the socket directory, read-only. Its fixed public-name host mapping validates source policy without public DNS or traffic. Launcher confinement and private-origin policy remain unchanged.

Gateway and worker use UID/GID 10001. The networkless initializer owns the dedicated socket directory and sets mode 0700. The socket has mode 0600. Each backend or tenant needs its own volume. Sharing a volume between same-UID containers removes that isolation.

| Service | CPU | RAM | PIDs | Writable storage |
|---|---:|---:|---:|---|
| Gateway | 0.5 | 128 MiB | 32 | Dedicated socket directory |
| youtubei | 2 | 768 MiB | 96 | 448 MiB tmpfs |
| bgutil | 1 | 512 MiB | 64 | 64 MiB tmpfs |
| Worker | 2 | 512 MiB | 96 | 192 MiB tmpfs |

Application roots are read-only. Containers drop capabilities, prohibit privilege escalation and use an init process for signal forwarding and child reaping. The initializer restores only CHOWN and FOWNER. Quote each tmpfs mount string so YAML does not split comma-separated options into separate mounts.

The gateway and youtubei each admit two jobs with no queue. Overload returns 503 before body buffering. A dispatched `/pot` request with uncertain completion retains its gateway slot. Upstream bgutil does not promise cooperative cancellation. Restart both gateway and bgutil to recover such slots. Local validation and method failures release admission because they never start upstream work.

Positive integer settings reject malformed or out-of-range values at startup:

- Service: `MAX_CONCURRENCY` 1..=16, `MAX_QUEUE` 0, `JOB_DEADLINE_MS` 10..=270000, `FFMPEG_KILL_GRACE_MS` 10..=5000, `MAX_FILE_BYTES` 1 byte..=1 GiB, `MAX_SCRATCH_BYTES` 1 byte..=3 GiB, `JS_EVAL_TIMEOUT_MS` 10..=5000, and `PORT`.
- Gateway: `GATEWAY_MAX_CONCURRENCY` 1..=16, `GATEWAY_MAX_QUEUE` 0, and `GATEWAY_DEADLINE_MS` 10..=270000.

The service deadline defaults to 240 seconds. The gateway defaults to 250 seconds, below the adapter's 280-second outer bound. Worker deadlines and byte limits apply independently.

Scratch accounting includes inputs and ffmpeg output. Keep tmpfs capacity above the per-job budget multiplied by concurrency, with framing and metadata headroom. ffmpeg `-fs` can overshoot by a packet. The tmpfs capacity is the physical ceiling.

Downloads use backpressure, exclusive owner-only files and abortable SDK fetches. Cancellation terminates ffmpeg with TERM, then KILL if needed. Cleanup waits for child reaping before releasing the slot or deleting scratch. Errors omit credentials and SDK logging is disabled. Legacy GET-info and POST-download contracts remain. The native gateway rejects sidecar token rotation because credentials are immutable after admission.

## Commands

Run from the repository root:

```sh
just --justfile backend/justfile test-youtube-auth-package
just --justfile backend/justfile build-youtube-auth-review
just --justfile backend/justfile test-youtube-auth-gateway-node22
just --justfile backend/justfile test-youtube-auth-topology
just --justfile backend/justfile test-youtube-auth-worker
just --justfile backend/justfile pull-youtube-auth-bgutil
just --justfile backend/justfile test-youtube-auth-bgutil
```

Image builds belong to the coordinator. The topology runner performs no builds or pulls. Each run uses a unique project and volume. Teardown allows 360 seconds for dependency stop waves. It checks for residual containers, networks and volumes, with bounded retries for late creation after a failed start. Failure reports live under `backend/media-reports/` in mode-0700 directories with mode-0600 files. Reports remain ignored.

## Verified in the main worktree

- All three review images built successfully, including the native runtime with Rust 1.88.
- The exact synthetic topology passed under Node 22.16.0 and ffmpeg 5.1.6-0+deb12u1. The service container suite passed 11 tests. Its two gateway-only tests skip because gateway sources are absent from that image; both pass separately under Node 22.
- The native confined launcher delivered a nonempty clipped artifact through the fixed Unix socket. Worker byte-limit and observed-refresh rejection passed. Frame and log checks found no synthetic credential or remote scratch-path leakage.
- Real-ffmpeg cases verified deadline, disconnect and output-limit termination with child reaping. Service tests covered file, scratch and concurrency limits.
- Gateway tests verified zero queue, deadlines, upstream cancellation and local PoToken rejection without upstream dispatch. An isolated negative control restored the old admission predicate and reproduced permanent exhaustion with 503 instead of 502.
- Eight container-only Python authentication/PoToken tests and one adapter contract test passed.
- The pinned bgutil image answered its real v2 ping under UID 1000, a read-only root, bounded tmpfs and no network. This check does not issue a PoToken.
- Full topology teardown passed and verified that its resources were gone.

## Remaining acceptance

Public Google/YouTube authentication and actual PoToken issuance are not accepted by synthetic fixtures or ping checks. Live account operations require separate operator authorization. Public-provider payload compatibility and joint restart behavior still need verification.

The full native admission and Telegram delivery suites remain separate. They must cover refresh races, expiry, ban/version changes, quotas, replay, uncertain sends and cookie fallback. This launcher fixture does not substitute for those checks.

CPU, memory, PID and tmpfs limits are configured in Docker. These checks do not claim destructive OOM, PID-exhaustion or tmpfs-exhaustion testing. Production activation remains separate from component verification.
