# Cookie web / browser / Telegram contract

Native migration 19, `yoink-storage/migrations-turso/0019_cookie_web.sql`, installs
both cookie control tables and their indexes through the verified migration ledger.
`cookie_web.rs` and `cookie_oauth.rs` use this schema. `install_cookie_web_schema`
is a disposable-test hook. Production uses the migration ledger only.

Token response: `{token, expires_in:600, submit_url:"/api/v1/dl/cookies/submit"}`.
Tokens contain 244 random bits, stored only as SHA-256, multiple issued tokens
remain independently usable, explicit actor-owned revocation removes all.
Consumption and every domain write share one immediate transaction. Validation,
capacity and fresh authorization failures roll back all writes and preserve tokens.
Boundary expiry is exclusive (`now >= expires_at` rejects). Clock arguments make
regressions deterministic; HTTP and Telegram supply the current UTC time.

Web requests never trust body user_id or JWT role. Lists contain metadata, never
content. Pool inheritance is one valid row per domain and requires current admin
role/configured owner or unexpired shared_cookies grant. Mutations recheck current
ban/role and row ownership under the DB write lock. Imported ID, creation time,
label and avatar survive personal replacement. Existing content-based credential
versioning invalidates admitted references when content changes.

Telegram /cookie token uses the same durable issuer. Configured links are enabled
by `DownloadService::with_cookie_web_origin("https://host/")`: Manage cookies is
a WebApp link to the existing frontend's `/cookies` route. Telegram does not
emit a mobile `/cookie-sync` link because that page has no implementation. The origin
setter rejects credentials, query, fragment, non-HTTPS and non-root paths.
The native runtime reads `YOINK_RUST_COOKIE_WEB_ORIGIN`; leaving it unset hides
WebApp links. Invalid origins fail configuration validation without echoing values.
`YOINK_RUST_COOKIE_OAUTH_TIMEOUT_SECS` and `YOINK_RUST_COOKIE_ACCOUNT_TIMEOUT_SECS`
configure provider deadlines, defaulting to 15s and 10s with a 1..=60s bound.
Both providers and the cookie HTTP service are installed before readiness.

## OAuth credentials and device sessions

`OAuthCredential::decode_content` accepts actual imported `__oauth2__` JSON with
access_token, refresh_token and ISO expires_at. Serialization preserves this
legacy content kind in cookies.content; ordinary uploads still reject it using
strict Netscape validation. Credential Debug is redacted. OAuthReference is
`{cookie:CookieReference,kind:"youtube_tv_oauth_v1"}`; version reuses profile::cookie_version (content, ID, owner, domain, pool
and updated_at) with length-delimited components. Resolving secrets is a Rust-only
actor-owned operation, not a frontend content endpoint.

`oauth_credential(actor,now)` resolves a typed imported/new personal credential.
`oauth_refresh(actor,expected_reference,now)` calls the provider outside the DB
lock, then rechecks current actor authority/capacity and CAS-checks the complete
stored reference before writing. Native profile preparation now shares this DTO
and provider with the pre-admission refresh/CAS path. Worker execution uses the
fixed Unix gateway when the operator enables the adapter. Container and public
provider acceptance remain separate gates in `youtube-auth-integration.md`.

Device sessions are durable, hashed-ID, actor-owned, expiring and poll-throttled.
Foreign callers cannot poll Google. Provider polling occurs outside DB locks.
Service clones share one tracked ownership registry per database. Overlapping
polls are rejected. Accepted work survives HTTP cancellation and stages success
without allowing a later pending/error generation to discard it. Current actor,
session ownership and deletion checks still apply. A completed provider result
survives capacity rejection so retry does not lose a consumed device code. Successful cookie write + session deletion is atomic. Imported metadata is
preserved. `OAuthProvider` is injectable for deterministic offline tests; the
actual YouTube implementation uses source-proven fixed endpoints/public TV client,
HTTPS only, no redirects or environment proxies, bounded responses and timeouts.
Provider error bodies never appear in frontend errors.

Shared integration hunks: downloads module declarations/service fields; HTTP
AuthState installation/router merge/dependency; runtime service installer;
Telegram token Action + list keyboard; backend/justfile focused checks; lockfile.

The shared `yoink-worker-protocol::youtube_auth::YoutubeOAuth` DTO and protocol
module declaration are small coordinator integration hunks: reconcile them with
worker6127, retaining 16,384-byte per-token validation and 64 KiB whole credential.
`OAuthReference.cookie` is the existing domain CookieReference; its version is
computed by `profile::cookie_version`, not a management-only digest. Exposing
that function as pub(crate) is the only profile module hunk.

Timing configuration: `OAuthProviderConfig::new(Duration)` accepts 1..=60s
(default 15s), `YouTubeOAuthProvider::with_config`; account-provider equivalents
accept 1..=60s (default 10s). Their typed configuration remains available to the
core-config integrator. No provider network call holds a database transaction.
Live validation supports exactly source-backed YouTube/Google, Instagram and
Twitter/X endpoints, merges bounded rotated existing cookie records, and CAS
checks full content/updated_at with fresh ban/role/ownership before writing.
Pool label refresh is authenticated admin-only, with source-backed hint fallback.

Provider network confinement also pins DNS results to conservative public-only
addresses (rejecting mixed public/private answers) and allowlists exactly the
three source-proven API hosts. DNS blocking-work permits survive cancellation;
requests never use a redirect target, environment proxy or caller-chosen host.

## Implemented routes

All private routes use authenticated actor IDs and current database authority.
Base path: `/api/v1/dl/cookies`.

| Method | Suffix | Contract |
|---|---|---|
| GET | (base) | Own metadata plus currently authorized inherited valid pool metadata |
| POST | (base) | Strict Netscape actor-owned upsert; legacy body user_id ignored |
| POST | `/upload` | Strict Netscape upsert with authentication-cookie hint check |
| DELETE | `/{domain}` | Actor-owned personal domain removal |
| DELETE | `/by-id/{id}` | Owned personal row removal; current admins may remove any row |
| GET | `/all` | Current admin-only complete metadata list |
| GET | `/pool?domain=...` | Current admin-only pool metadata, optional domain filter |
| POST | `/pool` | Current admin-only bounded validated pool add/dedup/update |
| DELETE | `/pool/{id}` | Current admin-only pool removal |
| POST | `/pool/refresh-labels` | Current admin-only live label extraction, `{updated}` |
| POST | `/{id}/validate` | Owned personal/current admin live validation and bounded rotation |
| POST | `/token` | Actor-owned durable sync token issuance |
| DELETE | `/token` | Explicit actor-owned revocation of outstanding sync tokens |
| POST | `/submit` | No JWT required: hashed single-use token plus browser cookie map; 204 |
| POST | `/yttv/start` | Owned durable device session; unchanged start response fields |
| GET | `/yttv/poll/{session}` | Owned throttled poll; pending/expired/error/ok statuses |

There is no executable content/export HTTP route or separate PUT/PATCH cookie
route in the reviewed actual router/client. CookieResponse deliberately excludes
content. No speculative secret-returning endpoints were invented. Existing
content replacement is POST upsert; OAuth secret access is a typed Rust-only
owner-authorized interface. The legacy Python handler links to `/cookie-sync`, but the
tracked unchanged frontend has no `/cookie-sync` page. Token responses therefore
contain no mobile upload URL. The `/cookies` WebApp link and browser extension
submission remain available. The operator must serve the frontend and API at the
configured HTTPS origin. The native JSON router does not serve the SPA.
REST checks do not verify the browser or reverse-proxy configuration.

Durable control-state bounds: 128 active browser tokens per actor / 100,000 total;
8 active device sessions per actor / 10,000 total. Rejection preserves existing
usable credentials; expired records are evicted. Pool storage has a finite 1 GiB
bound, with imported over-limit shrinking/replacement supported without growth.

## Runtime polling lifecycle

`YOINK_RUST_COOKIE_OAUTH_POLL_BUDGET_SECS` bounds accepted work, including DB waits, provider I/O and persistence. Its default is 120 seconds, with a 61..=294 runtime range. The runtime caps shutdown at 300 seconds, so the poll budget must leave room for the drain margin. `YOINK_RUST_COOKIE_OAUTH_MAX_INFLIGHT` defaults to 128, with a 1..=10000 range.

`YOINK_RUST_COOKIE_OAUTH_RESULT_RETENTION_SECS` gives newly staged results a fixed retry window. Its default is 1800 seconds, with a 60..=86400 range. Retries do not renew this window. Accepted polls temporarily protect their session from expiry cleanup.

The runtime periodically calls expiry cleanup. `YOINK_RUST_COOKIE_OAUTH_CLEANUP_INTERVAL_SECS` defaults to 60 seconds, with a 1..=3600 range. Shutdown closes poll admission and drains accepted tasks before closing the store. Media-enabled shutdown defaults to 135 seconds. `YOINK_RUST_SHUTDOWN_TIMEOUT_SECS` must exceed the poll budget by more than five seconds. Container stop grace must cover that shutdown deadline.

An unavailable DB beyond the accepted-work budget, or forced process termination, can still lose an unstaged provider response. Ordinary Netscape validation rejects OAuth content without changing its validity, content, timestamps or reference version.

## Main-worktree verification

`just test-cookie-web-regression` passed 438 tests across downloads, HTTP, server, storage and Telegram. `just check-cookie-web` passed all-targets Clippy with `-D warnings`. The server lifecycle regression verifies periodic expiry cleanup and persistence after request cancellation, before store close. Removing the runtime drain made this regression fail. Restoring the exact source made it pass. All fixtures use disposable local databases and fake providers. Container topology and public-provider acceptance are not covered by these checks.

`just test-personal-controls-native` builds and mounts the current backend, fixture server and worker into disposable local containers. It verifies ordinary-user Telegram document/URL imports, the actual `/cookie token` keyboard, JWT-free single-use browser submission, and extraction with the submitted jar through an authenticated proxy and directly. The source rejects unexpected cookies, so the browser cookie with an unrelated path must remain unsent. Replay, revoked authority, callback ownership and unchanged foreign/pool state also pass. Fixture diagnostics redact the issued sync token. This does not verify the frontend, reverse proxy, or public providers.
