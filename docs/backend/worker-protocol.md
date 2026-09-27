# Worker protocol boundary

[yoink-worker-protocol](../../backend/crates/yoink-worker-protocol/src/lib.rs) defines versioned JSON messages for isolated media and content executors. It does not launch processes or implement a worker transport yet.

An operation is a validated request, not mutable Telegram conversation state. Worker messages carry a protocol version, operation UUID, and positive attempt number. Retries preserve operation identity and change the attempt fence. Consumers must reject results from an obsolete attempt rather than attaching them to whichever operation is currently active.

## Decode and logging rules

Use the crate's bounded frame encoder and decoder instead of deserializing envelopes directly with Serde. Frames are limited to 64 KiB, and envelope validation is part of the wire contract. A valid JSON object is not necessarily a valid worker message.

Source URLs can contain private query parameters. They are redacted in Rust `Debug` output but necessarily remain in persisted requests and worker payloads. Do not log raw frames, serialized requests, or unfiltered worker stderr. Protocol validation does not make payloads safe to expose in telemetry.

## Artifact trust boundary

Artifact paths are relative and lexically validated. That alone does not make them safe to open. The eventual consumer must enforce canonical workspace containment, reject symlink escapes, enforce byte limits, and clean up only files owned by that operation. A remote worker cannot authorize arbitrary host filesystem access by returning a path.

Network destination validation, DNS rebinding protection, redirect checks, subprocess deadlines, and cancellation are executor responsibilities. A syntactically valid HTTP URL is not an SSRF policy. The current contract does not promise those protections before the executor exists.

## Persistence boundary

The [operation queue](persistence.md) fences database transitions using the operation, attempt, worker ID, random lease token, and live expiry. The wire operation/attempt identity is not a replacement for that lease. The Rust process owning the lease must validate messages before recording success or failure.

Cancellation intent is not executor acknowledgement. Likewise, a worker result does not prove a downstream upload was recorded atomically. Recovery needs explicit handling of external effects and must not claim exactly-once delivery from PostgreSQL fencing alone.
