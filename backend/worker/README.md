# Python media worker

The worker consumes one version 2 JSON request from stdin and emits version 2 NDJSON events on stdout. Each process handles one attempt in an empty private workspace. A successful terminal event contains an ordered, bounded artifact list. Classified download failures emit one failure event and exit successfully. Invalid envelopes and invalid configuration emit no events and exit unsuccessfully. Library diagnostics do not belong on stdout, and the production entrypoint suppresses stderr logging.

Protocol version 2 also accepts a distinct metadata-only `youtube_search` envelope, with no `source_url`, options or cookies. `search_engine.py` uses pinned flat YouTube extraction with downloads disabled and emits one bounded `search_completed` event with zero through eight validated canonical watch URLs. Search shares the guarded network/proxy stack and executor lifecycle; it creates no artifacts. Existing download/content envelopes and queued replay remain unchanged. See [native inline search](../docs/inline-search.md) for exact fields, limits, operator egress authority, configuration and focused fixtures.

`main.py` validates the envelope and configures networking. `media_engine.py` uses pinned yt-dlp and gallery-dl libraries for video, audio, clips, selected playlist entries, image batches and optional gallery ZIP output. `subtitle_engine.py` retrieves standalone SRT/VTT tracks through the same guarded yt-dlp HTTP stack without downloading video. Arbitrary downloader arguments, ambient configuration, plugins, browser cookies and downloaded executable components are disabled. Content extraction is not implemented. Python-native adaptive downloads may run, but external FFmpeg networking is prohibited. End-to-end adaptive-streaming coverage remains required before claiming compatibility.

FFmpeg and ffprobe run through immutable image-owned wrappers under `worker/bin/` that restrict protocols to `file,pipe` and allow only selected local demuxers. No executable files are created in the noexec workspace. Their execution and output are bounded. The engine probes output before producing artifact metadata. The Rust executor independently checks file types, sizes, aggregate limits and open-file identity before handing artifacts to delivery.

Standalone subtitles require `media_mode: "subtitles"` and `options.subtitles: {"language":"en","automatic":false}`. Both fields are required, with no unknown subtitle keys. Tags are exact bounded language codes, not regexes. English selects only `en`, while other languages also select their exact `-orig` track. Manual tracks take priority over automatic ones. At most two UTF-8 files are returned, preferring SRT over VTT, with a 4 MiB per-text ceiling plus the configured artifact/aggregate/item limits. Timing and text validation reject empty, malformed and binary output. Clips, playlists, gallery and encoding options cannot accompany this mode. The Rust executor checks signatures and safe language-bearing filenames rather than invoking ffprobe on text. Embedding, burn-in and ASS/SSA are not supported.

Source format reports require `media_mode: "formats"`; all other media-selection options must remain at their defaults. They use guarded yt-dlp metadata extraction with downloads and postprocessors disabled, reject playlists and live sources, omit DRM formats, and return one `text/plain` `formats.txt`. The report has a fixed TSV schema, at most 100 rows and a 64 KiB ceiling. Metadata strings are mapped to fixed enums or placeholders and never copied into the report. This mode does not accept format selectors.

Adaptive VOD fixtures cover HLS MPEG-TS and separate-stream DASH downloads. yt-dlp retrieves segments through the guarded HTTP layer, with unavailable-fragment skipping disabled. FFmpeg/ffprobe can read downloaded local MPEG-TS, but cannot open HLS/DASH/concat manifests or use network protocols. `just test-media adaptive` verifies full-length video, extracted audio, cache/restart and failure before delivery for missing fragments, private redirects and private HLS keys. Live streams, successful encrypted playback and DRM are not covered.

## Source prose and content policy

Media descriptors also carry `metadata: {title, description, tags, categories}` from actual extractor info dictionaries. `content_metadata.py` supplies bounds, not NSFW classification: there are no rules, policy decisions, role claims, or trusted `is_nsfw` flags in the worker. The Rust protocol rejects unknown metadata fields and independently checks bounds. Metadata Debug output is redacted.

Title is bounded to 2000 Unicode characters; description has the legacy 2000-character cap. Tags and categories each allow at most 100 strings of at most 128 characters. Multi-item batches conservatively retain the combined, deduplicated source prose for all outputs (including clips and gallery ZIPs). Aggregate overflow fails with `resource_limit`; it never silently drops a later item's classification evidence. Missing extractor metadata fails rather than claiming a safe result. Standalone subtitle descriptors carry the same source prose. Fixed-schema source format reports remain outside media-content classification.

Rust retains this metadata with confirmed cached Telegram file IDs and reevaluates current rules and destination policy before each delivery unit. Old workers and caches lacking metadata cannot silently bypass the policy: old caches are misses/refetched, and fresh media without metadata is refused. See [native content policy](../docs/nsfw-policy.md).

## Isolation and networking

The experimental runtime launches `/usr/local/bin/yoink-worker-launcher`, which requires Linux Landlock ABI 6 or newer. It grants write access only to the attempt workspace, read access to required runtime paths, and restricts signals and abstract Unix sockets. Unsupported kernels fail closed. This is filesystem and IPC confinement, not a kernel network sandbox.

The Python network guard validates source URLs and every urllib/requests redirect, rejecting private, loopback, link-local, multicast, reserved and mapped-private addresses unless the exact source hostname and port is explicitly allowed. The source allowlist does not authorize unrelated redirects.

A supplied HTTP or HTTPS proxy goes through a local CONNECT relay. The relay resolves each destination itself, checks every returned address, then sends a numeric-IP CONNECT request to the upstream proxy. A distinct proxy allowlist permits operator-approved private proxies without granting source access to them. Direct requests to the local relay and bypassing it through the upstream proxy are rejected. These checks protect the configured download paths, not arbitrary Python programs.

Network credentials enter only through the request's redacted stdin section. They do not belong in arguments, environment variables, logs or persisted job JSON. Netscape cookies use a temporary mode-0600 file inside the workspace and are removed on completion or failure. Gallery cookie writeback is disabled. Runtime credential selection, OAuth refresh and cache authorization belong to the Rust service and are not implemented by this worker.

## Worker environment

The Rust runtime may impose tighter limits than the worker's standalone bounds.

- `YOINK_WORKER_MAX_BYTES` limits each artifact to 1 through 2147483648 bytes. The standalone default is 52428800.
- `YOINK_WORKER_TOTAL_MAX_BYTES` limits aggregate artifact bytes to 1 through 8589934592. The standalone default is 52428800.
- `YOINK_WORKER_MAX_ITEMS` limits artifacts to 1 through 100. The standalone default is 20.
- `YOINK_WORKER_SOCKET_TIMEOUT_SECS` limits network waits to 1 through 300 seconds. The standalone default is 30.
- `YOINK_WORKER_PROCESSING_TIMEOUT_SECS` limits each local media-processing command to 1 through 86400 seconds. The standalone default is 300. The executor also imposes a whole-attempt deadline.
- `YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS` and `YOINK_WORKER_ALLOWED_PROXY_ORIGINS` are separate JSON arrays, both empty by default. Origins contain only scheme, hostname and optional port, with no credentials, path, query or fragment.

Budget failures are terminal `resource_limit` events. Expected HTTP failures map to fixed categories, including `rate_limited`, `invalid_media` and `source_unavailable`. Exceptions never expose source URLs or credentials through protocol errors.

## Verification

Python runs only in the experimental container. Tests mount the worker directory read-only and use loopback fixtures with container networking disabled.

```sh
cd backend
just test-worker
just test-worker test_gallery_integration.py
just test-worker test_proxy_tunnel.py
just test-worker test_subtitle_engine.py
just test-worker test_format_engine.py
# Existing image only; does not rebuild a shared image tag:
just test-nsfw-worker
just test-nsfw-worker 'test_*.py'
```

## Versioned provider routing

Rust, not Python or a Telegram URL parser, owns the automatic table. At admission it
turns automatic intent into `auto_yt_dlp_v1`, `auto_gallery_v1`, or
`auto_fallback_v1` in the immutable request. Explicit `yt_dlp` and `gallery` remain
explicit intent. These typed engine variants enter persisted JSON and cache hashes.
`auto` on an older stored job retains its previous worker behavior; no decoding-time
rerouting is performed. Subtitles and format reports remain yt-dlp only.

Gallery handles applicable ordinary video requests and mixed image/video batches.
It returns finalized outputs in extractor order, with a shared batch ordinal across
child extractors, rather than filtering a directory for image extensions. Explicit
image requests also keep video returned by the gallery extractor. Probe/processing,
artifact bounds, native delivery grouping, credentials and Rust NSFW policy are not
bypassed.

Instagram's automatic fallback runs each provider in a private attempt subdirectory.
Only the fully successful attempt's artifacts and source prose are published. A
metadata/extraction or download provider failure may try gallery; successful primary
extraction/download suppresses fallback. Both-provider failure publishes nothing.
Resource limits, confinement/network-policy denials, processing failures and
cancellation cannot enable fallback. Processing an already downloaded primary is
not a provider-failure escape hatch. These are approved repairs to legacy's broad
exception fallback. Gallery writes are checked against the attempt directory before
opening them. The outer launcher/executor deadline and confinement still apply.

Fallback eligibility is default-deny, independent of diagnostic text. Only recognized
extractor/HTTP/download failures inside the actual yt-dlp call can opt in. This includes
extractor errors with a typed `json.JSONDecodeError` cause, which yt-dlp marks unexpected
when the provider returns malformed JSON. Local wrappers still prohibit fallback.
Source validation, tool/cookie/configuration setup, local metadata/progress callbacks,
unexpected implementation errors and postprocessing remain forbidden. Network guards
have structural error markers and a per-process monotonic failure counter, also used
when the CONNECT relay converts a guard/local error to an HTTP response and loses
its exception chain. yt-dlp's typed retry error is retained as a causal link while
forwarding its retry behavior unchanged; an opaque DownloadError alone is insufficient.

Focused verification without building shared images:

```
cd backend
just test-provider-routing
just test-provider-regression
just test-provider-worker
just test-provider-worker-regression
just test-provider-fixtures
just test-provider-native
```

The worker unit recipes use the existing `yoink/rust-backend:experimental` image
with current source mounted read-only at
`/worker`, no container network, and CPU/memory/PID limits. Loopback-only HTTP is
available inside the container. Regressions cover real local downloads, malformed
provider JSON, truncated-provider provenance and structural setup/callback/guard
failures. These do not assert live provider
compatibility.

The native `provider-routing` scenario uses unmodified pinned yt-dlp 2026.8.19 and
gallery-dl 1.32.13 Instagram and 4chan extractors. Disposable DNS pins source hosts
to a documentation address, an authenticated proxy maps only that numeric authority
to local TLS, and a disposable certificate supplies fixture trust. Instagram API
responses distinguish the libraries' existing X-ASBD-ID headers and require the
same selected session cookie; no worker strategy or native admission is mocked.

Native acceptance covers the routing table and deceptive domains/VK queries; actual
HTTP metadata failure, malformed JSON and truncated primary download fallback; primary success suppressing
gallery; both-provider failure without cache/delivery; ordered mixed media and explicit
image; primary byte limits, gallery item limits, forbidden redirects and cancellation;
revoked frozen cookies despite an available pool alternative, running/queued role
revocation; one immutable queued operation and one charge across restart/replay; and
fresh/cached NSFW prose, private spoilers, group/topic delivery and late policy revocation.
It compares uploaded bytes, retained cache metadata, native delivery journals, frozen
requests and quota counters. Failed-primary metadata cannot spoil a safe fallback.

`just test-provider-native` passes using source-mounted native binaries and the
existing images, without rebuilding shared tags. It builds only the server's two
entrypoints in this worktree's dev target. The test binaries link CRT statically:
a dynamic host build required GLIBC 2.38/2.39/2.44 absent from the Debian image. Both
migration and fixture binaries are mounted so fixture schema and source revisions
match; the existing image's migrated schema was newer than this worktree. Current
worker source is read-only at `/opt/yoink-worker` and still runs through the image's
confined launcher. This opt-in exists only in the focused test driver, not production.
Matching pinned release images and main's integration checks remain coordinator-owned;
run `just test-media provider-routing` against those images after integration.

### Persisted-contract rollback

The `auto_*_v1` enum values change persisted request JSON and the worker's typed
protocol contract without increasing the database schema or NDJSON envelope version.
An older binary cannot safely decode/resume these operations **even if its database
schema check succeeds**; an older worker cannot consume the new engine values either.
Keep runtime and worker revisions paired. Use a compatible reader or a pre-routing
backup for rollback, never rewrite frozen requests, delivery receipts or uncertain
operations to force an older binary to resume them. No migration is added by routing.
