# Native downloader inline lookup

Inline text searches YouTube using the pinned yt-dlp search extractor. Each card
is a text-only article. Selecting it inserts a visible canonical YouTube watch
URL with automatic link previews disabled. Direct HTTP(S) URLs produce the same
article type and preserve URL bytes and supported clip tokens. No card contains
thumbnails, media previews or cached Telegram file IDs. Reserved music hosts are
not claimed by the downloader. The Stats /search command is unchanged.

The query actor is resolved from InlineQuery.from. Current registered dl:inline
grants are read without a destination, before lookup and again before answering.
Bans win over configured ownership and explicit grants. chat_type is not a chat
ID or authority. Unknown actors and database failures fail closed. Every answer
is personal with cache_time=0. Queries perform no admission, quota, media-cache,
deferred-media, NSFW-mark, quality-draft or cut-draft mutation.

Selection is an ordinary actual Telegram message. Existing private/group/topic,
role/ban, NSFW, preference, credential, cache, quality/cut and immutable replay
checks remain authoritative. An inline grant does not grant destination access.
The actual private /start search_help command checks current inline authority
before normal registration-pending handling, so explicitly inline-granted
restricted users can read help without changing registration state or acquiring media permission. Revoked or banned actors receive the usual denial. All UI strings
are yoink-i18n keys in the EN/RU catalogs.

## Worker contract and confinement

Protocol version remains 2. Existing download/content envelopes, options and
terminal semantics remain unchanged, including queued operation replay. Search
is a distinct typed SearchRequest envelope:

```json
{"version":2,"operation_id":"12345678-1234-1234-1234-123456789012","attempt":1,"task":{"kind":"youtube_search","query":"cats"},"network":{"proxy_url":null}}
```

Search forbids source_url, options, cookie jars and unknown task/network fields.
It accepts 2 through 256 clean Unicode characters (at most 1024 UTF-8 bytes).
A successful terminal event is search_completed with zero through eight unique
SearchEntry objects: url, title, channel, duration_seconds and view_count. URLs
must exactly match https://www.youtube.com/watch?v= plus an 11-character YouTube
ID. Titles/channels are sanitized and capped at 120/100 characters. Numeric
metadata is bounded. Rust independently validates every returned entry. Search
frames have a 32 KiB ceiling. HTTP metadata bodies have a 2 MiB per-response
ceiling and at most sixteen requests. Unexpected artifacts are refused.

Search shares the media executor's launcher, private workspace, cleared
environment, guarded HTTP/redirect/DNS stack, separate source/proxy allowlists,
output/workspace monitoring, process-group cleanup, cancellation and deadline.
Flat metadata extraction uses download=False, no postprocessors, no cache,
plugins, ambient config, browser cookies, thumbnail writes or media downloads.
Policy denials and configured proxy failures cannot fall back to direct egress.

Only operator-wide DOWNLOAD_PROXY_URL is search egress authority. Personal
proxy settings, cookie pools, OAuth modes and destination credential selection
are not consulted. The pinned extractor may create its own public language and
consent cookies, but no user authentication cookies are supplied. A missing
operator proxy selects guarded direct egress; a supplied proxy is mandatory.
The Config consumer already adds YOINK_RUST_: the previously doubled-prefix
network spelling is retained as an alias. Conflicting current/legacy values now
fail closed instead of choosing an authority silently.

## Operational configuration

See [consumer-parsed example](inline-search.env.example). Existing configs keep
these defaults:

- YOINK_RUST_TELEGRAM_INLINE_CONCURRENCY=3, range 1..8.
- YOINK_RUST_TELEGRAM_INLINE_TIMEOUT_SECS=20, range 1..120.
- YOINK_RUST_TELEGRAM_INLINE_REQUESTS_PER_SECOND=10, range 1..100.
- YOINK_RUST_TELEGRAM_INLINE_ACTOR_INTERVAL_MS=300, range 1..10000.

One absolute deadline starts at query admission and covers authorization, language lookup, extraction, reauthorization and the Telegram answer. Extraction receives the remaining budget, capped by the existing download worker deadline. Expiry cancels the query and awaits worker cleanup. It cannot start another answer timeout.

The transient actor map and tracked tasks each have a 1024-entry ceiling. Task capacity includes throttled responses and pending worker cleanup. Search workers have no pending queue. Exhausted worker concurrency returns a localized busy answer when response capacity is available. New queries cancel earlier queries for that actor, including throttled queries. All response paths run outside polling handlers in tracked tasks. Adapter shutdown cancels and waits for these tasks. Answer failures never mutate admission state.

## Focused checks and coordinator acceptance

```sh
cd backend
just test-inline
just test-inline-worker
just test-inline-wire
just test-crate yoink-server
just test-crate yoink-downloads
just check-inline
node --test scripts/media-fixture/inline.test.mjs scripts/media-fixture/bot-api.test.mjs scripts/media-fixture/gallery-source.test.mjs
```

Python runs only inside the existing pinned Docker image. The wire test is an
opt-in Rust encode -> pinned Python parser/engine and actual YouTube extractor
against loopback HTTP metadata -> Rust typed decoder test. Its test-only
transport remaps HTTP requests, not the extractor or returned cards. It refuses
media processing. Separate tests cover real pinned video metadata for selected
watch URLs, response overflow and proxy failure without direct fallback.

Coordinator-owned matching images and native acceptance:

```sh
cd backend
just build-media
just build-media-fixture
just test-media inline
```

The native fixture retains real YouTube URLs through the confined launcher. Its
operator proxy routes fixture-only numeric CONNECT destinations to a local TLS
metadata server with an explicit source allowlist and fixture CA. It runs real
polling, extraction and selection, plus grant revocation, denied actors,
supersession, capacity/rate limits, timeout/shutdown, query/answer state safety,
audio/image arms, immutable replay, compatible cache, NSFW private blur and group
refusal, real topic denial and owner quality/clip/cut interactions. Ordinary-user
quality/cut control generalization is integrated separately by the coordinator.

The held-worker shutdown check sends SIGTERM and requires normal exit. The original fixture process records live children and workspaces before cancellation, then zero children and workspaces after adapter draining, before container exit destroys tmpfs. Forced removal or an empty replacement container does not satisfy the check.

Matching-image native acceptance is required before claiming that fixture passed. These changes do not establish full downloader parity or deployment
completion.
