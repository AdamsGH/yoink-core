# Worker protocol

[yoink-worker-protocol](../../backend/crates/yoink-worker-protocol/src/lib.rs) defines typed requests and events for confined media, content, search, Music, and Inbox processing.

The protocol crate validates messages. It does not launch processes or authorize a Telegram destination.

## Identity and bounds

Messages carry a protocol version, operation UUID, and positive attempt number. Retries retain operation identity and advance the attempt fence.

The executor rejects stale-attempt results, unexpected terminal events, unknown fields, oversized frames, and invalid artifact descriptors. Use the bounded protocol codec rather than raw JSON deserialization.

Version 2 alone does not prove compatibility. New request variants can require a matching runtime and worker.

## Request families

- Media requests cover supported video/audio/image processing, playlists, clips, subtitles, format reports, and stream-link reports.
- Inline search requests return bounded metadata without downloading media or consuming personal credentials.
- Content requests retrieve guarded text and transcripts for Insight.
- Music requests resolve providers and optionally produce confined audio artifacts.
- Inbox requests provide bounded content and classification primitives.

The [worker implementation](../../backend/worker/main.py) dispatches these requests. Product services select credentials, enforce quotas, and authorize effects outside the worker.

## Secrets and diagnostics

Source URLs can contain private query parameters. Persisted requests may retain those URLs. Rust diagnostic formatting redacts sensitive fields.

Credential values travel in bounded stdin fields. They do not belong in process arguments, inherited environment variables, raw frame logs, or unfiltered worker stderr.

Personal cookie files are private workspace artifacts. Runtime services retain references and versions rather than embedding cookie contents in ordinary operation metadata.

## Confinement

The fixed launcher clears the environment and enforces Linux Landlock restrictions. Startup fails when required confinement is unavailable.

Guarded HTTP clients check DNS results and redirects. Proxy origins have separate authority from source origins. A permitted proxy does not authorize arbitrary private destinations.

FFmpeg and ffprobe process local files through fixed wrappers. External media-tool network access, arbitrary downloader arguments, ambient configuration, and plugins are not supported.

## Artifacts and delivery

Artifacts remain inside the attempt workspace. The executor checks regular-file identity, symlinks, hard links, MIME/signatures, item counts, sizes, and aggregate budgets before delivery.

The delivery service rechecks current access, credential references, and content policy. Worker success does not authorize sending an artifact.

Database fencing protects state transitions, not external effects. Confirmed receipts survive recovery. Unknown sends do not trigger automatic resends.

See [persistence](persistence.md), [media fixtures](../development/media-fixtures.md), and [worker development](../development/workers.md).
