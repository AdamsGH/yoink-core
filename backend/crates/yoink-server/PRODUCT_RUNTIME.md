# Native Music and Stats composition

Both products are opt-in. Set `YOINK_RUST_MUSIC_ENABLED=true` and
`YOINK_RUST_STATS_ENABLED=true` only with explicitly configured Telegram and
database dependencies. Normal startup verifies the required schema; it never
creates or migrates the database. Configure private, dedicated workspace,
index and import directories before enabling the products.

HTTP diagnostics bind before dependency startup. Readiness remains false until
factories, authenticated routes and Telegram menus are installed. A single
late Music/Stats holder contains Music account statistics, Stats routes and the
shared Activity registry. Insight and Music install different Activity keys.
HTTP, raw updates, commands and scheduled jobs use the same native identity,
database and Stats instance.

## Configuration

Every name below has the `YOINK_RUST_` prefix. Disabled products do not parse
their operational settings.

Music preserves the active legacy defaults: `MUSIC_REQUEST_TIMEOUT_SECS=10`,
`MUSIC_CACHE_TTL_SECS=3600`, `MUSIC_DOWNLOAD_ENABLED=false`. Spotify and Tidal
each accept optional `MUSIC_{SPOTIFY,TIDAL}_CLIENT_ID` and
`MUSIC_{SPOTIFY,TIDAL}_CLIENT_SECRET`. Credentials and proxy configuration
are never included in Debug or configuration errors.

`MUSIC_PROXY_URL` and comma-separated `MUSIC_PROXY_PLATFORMS` route only the
named providers through that proxy. Provider keys are spotify, deezer, yandex,
ytmusic, soundcloud, apple_music, bandcamp, qobuz, tidal and youtube. The worker
still requires explicit origin confinement via
`MUSIC_WORKER_ALLOWED_PROXY_ORIGINS` or
`MUSIC_WORKER_ALLOWED_PRIVATE_ORIGINS`, bounded JSON origin arrays.
There is no configurable worker executable or command line.

Other Music controls and defaults:

| Control | Default | Range |
| --- | --- | --- |
| MUSIC_LOOKUP_TIMEOUT_SECS | 30 | 1..120 |
| MUSIC_RESOLVE_TIMEOUT_SECS | 120 | 1..3600 |
| MUSIC_AUDIO_TIMEOUT_SECS | 120 | 1..3600 |
| MUSIC_DELIVERY_TIMEOUT_SECS | 30 | 1..60 |
| MUSIC_PERSISTENCE_TIMEOUT_SECS | 10 | 1..120 |
| MUSIC_CACHE_ENTRIES | 1000 | 1..10000 |
| MUSIC_WORKER_CONCURRENCY | 2 | 1..32 |
| MUSIC_WORKER_RESPONSE_BYTES | 1048576 | 1024..8388608 |
| MUSIC_MAX_REQUESTS | 30 | 1..100 |
| MUSIC_AUDIO_MAX_BYTES | 51380224 | 1..51380224 |
| MUSIC_MAX_LINKS | 10 | 1..10 |
| MUSIC_WORKSPACE | /tmp/yoink-music | absolute dedicated directory |
| MUSIC_WORKSPACE_MAX_BYTES | 209715200 | audio maximum..1073741824 |
| MUSIC_WORKER_SOCKET_TIMEOUT_SECS | 15 | 1..120 |
| MUSIC_WORKER_OUTPUT_BYTES | 1048576 | 65536..4194304 |
| MUSIC_OBSERVATION_MAX_AGE_SECS | 300 | 1..86400 |
| MUSIC_DELIVERY_RESPONSE_BYTES | 65536 | 1024..65536 |

Request timeout is bounded to 1..120 seconds and cache TTL to 1..86400 seconds.
The Executor's total and processing deadlines cover the larger of lookup and
audio budgets; the worker has at most one audio artifact per operation.

Stats controls and defaults:

| Control | Default | Range |
| --- | --- | --- |
| STATS_INDEX_DIR | /var/lib/yoink/stats-search | absolute dedicated directory |
| STATS_USERNAME_FIRST_DELAY_SECS | 60 | 1..86400 |
| STATS_REFRESH_INTERVAL_SECS | 3600 | 1..86400 |
| STATS_LOOKUP_TIMEOUT_SECS | 10 | 1..120 |
| STATS_SEARCH_REFRESH_INTERVAL_SECS | 3600 | 1..86400 |
| STATS_TEXT_TIMEOUT_SECS | 30 | 1..60 |
| STATS_LOOKUP_RESPONSE_BYTES | 65536 | 1024..1048576 |
| STATS_IMPORT_ROOT | /var/lib/yoink/stats-import | absolute dedicated directory |
| STATS_IMPORT_MAX_BYTES | 16777216 | 1024..16777216 |
| STATS_IMPORT_MAX_MESSAGES | 100000 | 1..100000 |
| STATS_IMPORT_MAX_JOBS | 16 | 1..128 |
| STATS_IMPORT_TIMEOUT_SECS | 120 | 1..3600 |

Username refresh preserves legacy first-delay and interval semantics. Search
refresh is a separate native derived-state job; its default matches the
hourly refresh cadence. Missed ticks delay rather than catch up. Legacy
declared-only group IDs, retention and chart controls are not invented here.

## Ownership and shutdown

The Products owner exists outside the cancelable startup future. It retains
the launcher preflight child, Music service, Executor, Telegram handlers,
Stats and its search TaskTracker. Factory cancellation before Stats
publication does not lose ownership of blocking index work.

The root also retains the adapter's generic handler lifecycle immediately
after prepare, before consuming any adapter builder. Shutdown clears readiness,
stops HTTP and generic Telegram admission, cancels adapters/jobs and joins
them under the existing shutdown budget. After adapter join, retained generic
messages/callbacks/inline handlers are canceled and drained. On timeout the
server cancels HTTP calls and aborts and joins adapters, then retries generic
handler and HTTP drain without abandoning ownership.
Only then does it drain Music handlers/provider work, reap the Music Executor,
stop Stats handlers and flush/close search. Even failed or unpublished startup
closes and waits for the retained search tracker. Database close is last.
Cleanup is not abandoned at an expired adapter deadline.

Stats import jobs are owner-only, ephemeral and confined to the configured
private root. Their retained manager stops admission, cancels and drains
every job before Stats shutdown, including failed or canceled startup.

## Verification

The parent owns the single workspace/all-feature build graph and artifact cap.
The child does not run Cargo. Parent commands:

```sh
cd backend
just check-build-space
just fmt check lint test
```

Source-named regressions in `src/product_runtime/tests.rs` exercise actual Stats
factory success, failed index startup, cancellation while a DB connection is
held, writer-lock release/reopen, retained unpublished blocking work and real
canceled Music preflight kill/join. Configuration tests cover opt-in defaults,
native dependencies, active legacy defaults, redaction and invalid bounds.
Actual-process/public product acceptance and deployment remain parent gates.
