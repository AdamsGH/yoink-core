# Worker protocol boundary

[yoink-worker-protocol](../../backend/crates/yoink-worker-protocol/src/lib.rs) defines versioned JSON request/event messages for media and content executors. The media executor is now connected to a fixed Python worker process, but the protocol crate itself does not launch workers.

An operation is a validated request, not mutable Telegram conversation state. Worker messages carry a protocol version, operation UUID, and positive attempt number. Retries preserve operation identity and change the attempt fence. The executor rejects results from an obsolete attempt.

## Decode and logging rules

Use the crate's bounded frame encoder and decoder instead of deserializing envelopes directly with Serde. Frames are limited to 64 KiB, and envelope validation is part of the wire contract. A valid JSON object is not necessarily a valid worker message.

Source URLs can contain private query parameters. They are redacted in Rust `Debug` output but necessarily remain in persisted requests and worker payloads. Do not log raw frames, serialized requests, or unfiltered worker stderr. The worker suppresses stderr to protect source URLs and library diagnostics.

## Implemented media contract

The fixed worker accepts progressive HTTP(S) video and produces one validated MP4 or WebM artifact. Audio-only, content extraction, HLS/DASH, and adaptive streams are unsupported. The worker disables yt-dlp plugins and configuration, cookies, proxies, and external downloaders, and uses Python networking. It resolves and checks destination addresses including redirects to limit SSRF. Private, loopback, link-local, multicast, reserved, and IPv4-mapped private destinations are rejected by default. An exact HTTP(S) origin allowlist exception is intended only for trusted isolated operator fixtures. This destination guard is not a container sandbox and does not establish safety for future protocols.

The executor launches the fixed image worker, caps process runtime, socket timeout, stdout, artifact size, and total workspace bytes, validates protocol identity and terminal event, verifies workspace contents and artifact path without following symlinks, and accepts only MP4/WebM magic bytes. ffprobe validation has a deadline, bounded streamed stdout, and discarded stderr. Process-group termination and reaping provide lifecycle cleanup, not a security boundary. Container hardening is still required; see the [media slice guide](media-slice.md).

## Artifact and persistence boundaries

Artifact paths are relative and validated beneath the per-attempt workspace. The consumer enforces canonical workspace containment, rejects symlink escapes, enforces byte limits, and cleans only its operation-owned files. A remote worker cannot authorize arbitrary host filesystem access by returning a path.

Network policy, subprocess deadlines, cancellation, and artifact checks are executor responsibilities. A syntactically valid HTTP URL is not sufficient by itself. The operation queue fences database transitions using operation, attempt, worker ID, random lease token, and live expiry. That fencing does not make external upload effects exactly once. See [persistence](persistence.md).
