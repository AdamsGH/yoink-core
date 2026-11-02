# Worker development

The worker accepts typed version 2 requests on stdin and emits bounded NDJSON events. Each process handles one attempt in a private workspace.

Classified request failures produce fixed failure events. Malformed envelopes or configuration fail without a successful terminal result.

## Task families

- Media extraction uses pinned yt-dlp and gallery-dl for video, audio, playlists, clips, and galleries.
- Subtitle extraction returns bounded SRT, VTT, ASS, or SSA text documents.
- Format and stream-link reports use metadata extraction without downloading complete media.
- Inline search returns validated YouTube metadata without personal cookies or media artifacts.
- Content tasks supply guarded extraction for Insight and Inbox.
- Music tasks resolve provider links and optionally produce tagged MP3 audio.

[main.py](../../backend/worker/main.py) dispatches requests. The [protocol crate](../../backend/crates/yoink-worker-protocol/src/lib.rs) defines their typed boundaries.

## Execution and network policy

The fixed launcher requires Linux Landlock ABI 6 or newer. Unsupported kernels fail closed.

It clears the environment, confines filesystem access, and restricts process interactions. It is not a complete kernel network sandbox.

Worker clients validate source addresses and redirects. Private proxy origins have separate authority from source destinations.

Credentials enter bounded stdin fields. Temporary Netscape cookie files use private permissions and are removed during cleanup.

Ambient downloader configuration, plugins, arbitrary executable arguments, and browser-cookie discovery are disabled.

## Local media tools

FFmpeg and ffprobe run through fixed image-owned wrappers. They allow selected local protocols and demuxers, not network manifests.

Adaptive VOD fixtures cover finite HLS MPEG-TS and separate-stream DASH. They do not establish arbitrary live-stream, encrypted-playback, or DRM support.

Subtitle documents are validated text. ASS/SSA tags are not rendered or executed. Embedding and burn-in remain separate unsupported workflows.

## Source metadata and artifacts

Media output includes bounded source prose for native content policy. The worker does not make a trusted NSFW decision.

The Rust executor independently validates descriptor bounds, artifact identity, paths, signatures, and workspace budgets.

Cached file IDs retain source metadata. The service reevaluates current content rules before delivery.

## Limits

The executor sets artifact, aggregate, item, socket, processing, output, workspace, and whole-attempt budgets.

Standalone worker settings use `YOINK_WORKER_*`. Native server configuration uses its typed `YOINK_RUST_*` controls. These namespaces are not interchangeable.

Budget failures and expected upstream failures map to fixed categories. Raw exceptions, credential contents, source query values, and library stderr must not enter public diagnostics.

## Container-backed tests

```sh
just --justfile backend/justfile test-worker
just --justfile backend/justfile test-worker test_proxy_tunnel.py
just --justfile backend/justfile test-worker test_subtitle_engine.py
```

Python runs inside the worker container. Tests use synthetic sources and owned temporary state.

Worker unit tests do not establish Telegram delivery or browser behavior. Use [media acceptance](media-fixtures.md), [Insight fixtures](fixtures/insight.md), and [Inbox fixtures](fixtures/inbox.md) for those paths.
