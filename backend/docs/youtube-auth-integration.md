# YouTube authentication execution handoff

The main runtime integrates bounded worker execution and pre-admission refresh. Container topology and public-provider acceptance remain open. Frontend and plugin sources are unchanged.

## Canonical credential contract (worker5588)

Use `yoink_worker_protocol::youtube_auth::YoutubeOAuth` and its `decode_content` / `encode_content` methods, not a parallel DTO/version implementation.

- `CREDENTIAL_KIND = "yttv_oauth"`.
- `OAUTH_CONTENT_PREFIX = "__oauth2__"`.
- `MAX_OAUTH_CONTENT_BYTES = 65_536` including prefix and encoded JSON.
- `MAX_OAUTH_TOKEN_BYTES = 16_384` UTF-8 bytes per access/refresh token.
- Imported content remains exactly `__oauth2__` + JSON `{access_token, refresh_token, expires_at}`. Unknown fields are rejected. Profile decoding requires RFC3339 expiry with timezone.
- The transient stdin field is `network.youtube_oauth`, mutually exclusive with `cookie_jar`, Debug-redacted and omitted when absent. It is not persisted in request snapshots, queue, artifact metadata or API responses.
- The reference remains the existing `CookieReference`. Management must reuse crate-visible `profile::cookie_version(content, id, owner, raw_domain, pool, updated_at)`; do not invent another OAuth hash. Content/type changes and metadata changes change this version.

`OAuthRefreshProvider::exchange(&YoutubeOAuth)` returns a boxed Send future yielding `Result<YoutubeOAuth, ProfileError>`. This is a **pure provider exchange**, not a DB mutation. Worker5588 supplies the real Google HTTP implementation; no device-start/poll APIs are duplicated here.

`DownloadService::with_oauth_refresh(Arc<dyn OAuthRefreshProvider>)` activates the refresh path already called by `ProfileStore::prepare_network`. `with_oauth_refresh_timeout(Duration)` configures the outer deadline (default 15 seconds, positive, maximum 60 seconds). Refresh is personal YouTube OAuth only and is skipped for no-cookie bypasses. A missing personal OAuth credential retains real Netscape cookie/anonymous fallback. A malformed/revoked selected OAuth credential is not silently replaced with cookies.

Expired/within-30-second credentials refresh **before admission**. Networking holds neither a DB transaction nor connection guard. After HTTP, the owner performs an immediate transaction: canonical version, row ownership/domain/pool/validity, current role/ban, current OAuth mode, then monotonic timestamp CAS. Concurrent revocation/replacement/demotion or auth-mode change stops admission. Provider timeout leaves the old row unchanged. Existing admitted references never invoke refresh; expiration, changed content/version, revocation or current authorization failure stops execution/delivery through existing rechecks. Worker-observed sidecar refresh is rejected and never persisted or silently delivered.

## Confined execution boundary

`Executor::with_youtube_auth_adapter()` enables exactly `/run/yoink-youtube-auth/adapter.sock`; no request controls this path. The launcher allowlists only that fixed operator socket value. No private-origin or DNS policy exception was added.

The standalone Node 22+ `worker/youtube_auth_adapter.mjs` serves Unix HTTP, mode 0600. Its upstream origins are **explicit operator configuration** only:

- `YOINK_YOUTUBE_AUTH_SOCKET=/run/yoink-youtube-auth/adapter.sock`
- `YOINK_TRUSTED_YOUTUBEI_URL=http[s]://operator-service:port` (origin only)
- `YOINK_TRUSTED_BGUTIL_URL=http[s]://operator-service:port` (origin only)

Only registered POST routes are accepted. The adapter bounds request JSON to 64 KiB, denies redirects, forwards neither upstream errors nor updated-token headers, and aborts upstream streaming when the worker disconnects. PoToken proxy/source-address/TLS-bypass options are denied; this implementation rejects proxied PoToken requests instead of silently bypassing proxy policy. No service URLs, remote filesystem paths, filenames or credential-management fields come from request options.

OAuth `/info` uses POST, not secret-bearing query parameters. Metadata is explicitly allowlisted into the existing collector. `/download` streams into a worker-generated exclusive local filename with per-file/total byte ceilings and a monotonic deadline. Response filenames/paths/title headers are ignored. Existing local ffmpeg/ffprobe wrappers then enforce current codec/container/height/clip behavior and artifact bounds. This sidecar supports one individual video/audio, not OAuth playlists, subtitle or formats reports; unsupported modes fail closed rather than inventing providers.

`youtube_auth_pot.py` registers an actual yt-dlp PoToken provider using the pinned provider API. It sends bgutil v2 `/get_pot` content-binding and innertube-context payloads through the adapter, checks `/ping` major version 2, and returns `PoTokenResponse`. GVS/player/subtitle contexts follow yt-dlp's binding helper. WEB_MUSIC without a captured challenge is explicitly rejected. Unconfigured installations preserve ordinary cookie yt-dlp execution, with no automatic localhost service contact.

## Separately identified integration hunks

1. Protocol: new DTO/module, optional redacted field and mutually-exclusive validation, `None` additions to existing Rust literals (including search, which still accepts no cookies/OAuth).
2. Profile: module registration, optional refresh provider/deadline, prepare/resolve dispatch using the existing reference/version, public canonical version function. Two previous blanket-unsupported assertions now assert missing-OAuth fallback.
3. Downloads: refresh-provider/deadline builders; network literal default.
4. Executor/launcher: fixed opt-in Unix socket environment propagation. No filesystem/origin allowlist expansion.
5. Worker main/media: bounded wire validation and dispatch into owned modules, existing processing path reused.
6. Existing youtubei service Docker source: POST `/info` addition while retaining legacy GET; credential-update listener installed before sign-in; refresh during info rejected; secret-bearing exception logging removed from download catch. Native gateway requires this patched service, not the existing public image's old GET-only API.
7. Justfile: focused test/clippy/format/worker recipes, one worktree target and resource preflight.

## Runtime integration and remaining packaging gates

The runtime reads `YOINK_RUST_YOUTUBE_AUTH_ADAPTER_ENABLED`, which defaults to false. Enabling it calls `Executor::with_youtube_auth_adapter()`. Users cannot change the socket or trusted service origins. The same configured Google provider serves management and pre-admission exchange, with `YOINK_RUST_COOKIE_OAUTH_TIMEOUT_SECS` bounding both. Management and execution share the DTO and cookie version function.

Remaining acceptance gates:
- Package the adapter in a dedicated Node 22+ container/process, supply explicit trusted service origins, and mount only its Unix socket directory into the worker runtime. Socket permissions/UID must permit the confined worker while excluding other tenants. Backend Dockerfile already copies Python worker sources; the adapter is not launched by the Python image.
- Package/rebuild the existing youtubei Docker service with the identified POST/refresh-observation hunk and pin youtubei.js/Node/bgutil artifacts. Do not reuse an unreviewed public tag as acceptance evidence.
- Bound trusted service concurrency, memory, CPU, PIDs, writable temporary-disk capacity and lifecycle. The legacy youtubei service buffers/downloads intermediates before response streaming; native artifact bounds do not by themselves bound that separate service's scratch disk. Cancellation/deadline cleanup of upstream jobs and ffmpeg must be accepted in the real container topology.
- Run deterministic end-to-end native admission -> confined launcher -> Unix gateway -> synthetic services -> real local processing -> delivery fixtures, including refresh races, expiry while queued/processing, byte/item/deadline rejection, ban/version rechecks, quota/replay/send-uncertainty regressions, cookie fallback and bgutil binding. Run the actual public container/provider acceptance separately under coordinator authorization. No production/live provider calls were made here.

## Verified checks

- `just test-youtube-auth`: exit 0, 148 Rust tests passed, one pre-existing search-wire Docker test ignored, 0 doctests; focused four-crate slice only.
- `just check-youtube-auth`: exit 0, all-targets Clippy with `-D warnings` for the four touched crates and resource preflight.
- `just test-youtube-auth-worker`: exit 0, 8 Python tests in the existing source-mounted Docker image and 1 Node gateway fixture test. Includes real local ffmpeg clip processing from synthetic streamed media, real registered PoToken provider/content binding, byte ceiling, refreshed-token denial, metadata filtering and fixed-route/redirect denial.
- `just test-provider-worker-regression`: exit 0, 114 Docker worker tests. No image build and no host Python.
- `git diff --check` and Node syntax checks: exit 0.

Temporary/pre-existing `backend/.cargo/config.toml` is excluded from integration. No staging, commits, pushes, full workspace/shared-image builds or live providers.
