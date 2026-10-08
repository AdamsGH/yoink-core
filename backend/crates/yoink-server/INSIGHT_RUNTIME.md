# Native shared Insight composition

## Ownership / handoff

The server constructs one full Insight service. HTTP, Telegram and enabled
Inbox receive clones of that service. Telegram receives the shared identity
store and durable text-delivery ledger before `initialize_menus()`. The adapter
owns dispatch, cancellation and draining. It does not need a separate Insight
run task. Persisted route settings remain module-owned.

`InsightRuntime::new(config, storage, identities, owner, shutdown)` constructs:

* the immutable `/usr/local/bin/yoink-worker-launcher` Executor + concrete
  `WorkerContent` (no executable/argument override, private-origin exceptions,
  host fetch, Python fallback or no-op content adapter);
* `HttpCompletion`, optional exact trusted OpenRouter-base operator proxy;
* `ResponsesResearch` via **Insight.with_research**, so each request supplies its
  current authorized research route, not a content-worker/global route resolver;
* committed access-change callback
  `Arc::new(move |_| identities.notify_committed_access_change())`;
* `DeviceOAuth::with_options` with trusted typed options, separate read and
  public_repo application IDs.

Standalone Insight is independent of Inbox and Telegram; an explicit DB is
required. IdentityStore's builtin Insight switch is true only for standalone
Insight. Inbox uses the exact service clone with `new_with_insight`; its provider
and content Arc identity is tested. When standalone Insight is off, Inbox retains
its completion-only service, using the same shared typed credential/route mapper.
The full loader maps Insight settings only once when both products are enabled.
The normal configured model is passed unchanged for module bootstrap fallback.
Native bot_settings remain module-owned and are reread per request.

## HTTP and activity hooks

`LateInsight::install(system, shared_auth, runtime.http(child_token))` installs
module-owned routes below `/api/v1/insight` after actual DB/schema preparation.
`serve_with_late_products` / `router_with_late_products` preserve existing auth,
media, Inbox, user-session and diagnostics routes; unavailable products are 404,
installed but unready product requests are 503. The bridge uses only the existing
`AuthState.service` + `authenticated_id`, inserts module `AuthenticatedUser`,
sets no-store and the shared Bearer challenge. No new JWT, role/cache or frontend
principal input is introduced. The module owns all authorization and redaction.

The baseline has no native activity registry/routes (the shared aggregation was
in Python). `yoink_http::activity::Activity` supplies the native authorized
aggregation seam, registers the **same Insight clone**, and exposes the legacy
`/api/v1/users/me/stats` and `/api/v1/users/{id}/stats` shapes. Viewing another
subject requires a freshly authorized Admin/Owner, before provider invocation
and again afterward. Both the viewer and the subject's current bans are enforced.
Native Insight log counts and timestamps populate ai_total/ai_last_at/plugins.
Missing providers have zero/empty legacy fields, not invented activity.

For additional native providers use `Activity::install(name, Arc<dyn
ActivityProvider>)` and `LateInsight::install_with_activity(...)`; the caller
must also `install_insight(runtime.service.clone())`. The registry clones handles
before awaiting providers, holding neither registry lock nor a DB connection
across provider I/O. No native DL/music provider existed to register in this
baseline; those future adapters can populate the existing dl/music fields.

## Shared configuration authority

All names below are read only by `Config::from_lookup`, with the standard
`YOINK_RUST_` prefix. There is no second environment, .env, or auth loader.
Disabled standalone Insight does not validate unused standalone settings.
Secrets, proxy credentials and OAuth endpoints are redacted from Debug/errors.

| Suffix | Default / constraints |
| --- | --- |
| INSIGHT_ENABLED | false; strict true/false, independent of INBOX_ENABLED |
| INSIGHT_GATEWAY_BASE / INSIGHT_GATEWAY_KEY | module defaults; base already includes /v1 |
| INSIGHT_OPENROUTER_BASE / INSIGHT_OPENROUTER_KEY | module defaults |
| INSIGHT_MODE | auto; auto/a2a/openrouter only |
| INSIGHT_MODEL / INSIGHT_SEARCH_MODEL | module defaults; normal and research model remain separate |
| INSIGHT_GEMINI_KEY / INSIGHT_GEMINI_MODEL | module defaults |
| INSIGHT_DEFAULT_LANG | en; en/ru/uk/de/fr/es/zh/ja |
| INSIGHT_TRANSCRIPT_LANGUAGES | JSON array; default ["en","ru"], 1..32 bounded language tags |
| INSIGHT_SUMMARY_DAILY_LIMIT / INSIGHT_TLDR_DAILY_LIMIT | 50 / 20; independent 0..1000000 |
| INSIGHT_CONTENT_MAX_CHARS | 40000; 100..200000 |
| INSIGHT_REQUEST_TIMEOUT_SECS | 120; 1..300 |
| INSIGHT_CONTENT_TIMEOUT_SECS | 60; 1..300, <= request |
| INSIGHT_TEXT_TIMEOUT_SECS | 15; 1..60, <= request; durable Telegram send/edit budget |
| INSIGHT_RESEARCH_TIMEOUT_SECS | 90; 1..300, <= request |
| INSIGHT_SOCKET_TIMEOUT_SECS | 15; 1..300, <= content |
| INSIGHT_RESPONSE_MAX_BYTES | 1048576; 1024..8388608 bounded provider/content response cap |
| INSIGHT_WORKSPACE | /tmp/yoink-insight; dedicated absolute existing directory, no traversal |
| INSIGHT_CONTENT_PROXY | optional explicit operator HTTP(S) proxy; exact origin passed to worker egress policy |
| INSIGHT_OPENROUTER_PROXY | optional explicit operator HTTP(S) proxy; never inherited by BYOK |
| INSIGHT_GITHUB_CLIENT_ID | Iv1.b507a08c87ecfe98 (read OAuth production default) |
| INSIGHT_GITHUB_PUBLIC_REPO_CLIENT_ID | optional separate public_repo app; absent write connect is module 501 |
| INSIGHT_OAUTH_REQUEST_TIMEOUT_SECS | 15; 1..300 |
| INSIGHT_OAUTH_USERNAME_TIMEOUT_SECS | 10; 1..request |
| INSIGHT_OAUTH_DEVICE_BASE | https://github.com |
| INSIGHT_OAUTH_USER_URL | https://api.github.com/user |

OAuth trusted endpoints permit HTTP only for loopback, reject userinfo/query/
fragment. Operator provider bases reject userinfo/query/fragment. Proxy userinfo
is allowed only for the explicit trusted proxy, never a provider base. BYOK's
pinned public DNS/no-proxy/no-redirect policy is unchanged.

Enable standalone Insight with an explicit shutdown budget: defaults require
`YOINK_RUST_SHUTDOWN_TIMEOUT_SECS=150` with the default five-second DB acquisition
budget. The maximum of whole-request and OAuth request+username budgets, plus
four DB acquisition budgets, must fit **strictly below** shutdown. Adapter
budgets must fit their request budget. Existing Inbox job/lease/drain constraints
still apply independently. Nothing operational reads an ad-hoc environment value
at runtime or embeds an unconfigurable new timeout.

## Lifecycle

The fixed launcher `--check` is verified before readiness. A concrete Child is
retained during preflight: startup cancellation/timeout/error kills and **awaits
reaping** before database teardown. A successful preflight is already reaped.
The full `InsightRuntime` is retained outside the startup future before route
installation. After HTTP/worker drain (including task-abort timeout), all normal,
startup-error and signal/error exits join OAuth.shutdown and
WorkerContent.shutdown, then await Insight.shutdown_quota_cleanup before DB
close. These joins are not aborted to race database close. Quota admission closes
across all clones; live reservations retain tracking through drop cleanup.
OAuth admission guards remove abandoned generations, and token persistence
checks cancellation, expiry, generation and current authority transactionally.
Worker shutdown fences subsequent full-content admissions.

## Locked integration checks

The coordinator reconciled only the HTTP and Telegram Insight edges in
Cargo.lock and added the non-retryable FullContent match arm in Downloads.
The integrated main checkout now passes its original locked checks without
disposable dependency repairs.

From `backend/`, `just check-crate`, `just lint-crate` and `just test-crate`
select one crate. Main Clippy passes with warnings denied for Downloads, HTTP,
Telegram and server. Test results are Downloads 137, HTTP 55, Telegram 166 and
normal server 43. `just test-fixture-server` passes 50 fixture-binary and
43 normal-server tests. The normal-server count overlaps the preceding run.
The existing proc-macro-error2 future-compatibility notice is not a test or
Clippy failure.

These checks establish source integration, current authority, shared handles
and deterministic wire regressions. They do not establish immutable-image,
confined-worker or built-SPA acceptance. Those gates require the coordinator
process fixture. OAuth start-admission and quota-cleanup lifecycle repairs now
pass 100 Insight tests, including nine new regressions. The concrete runtime
shutdown test verifies that the shared Inbox clone rejects later quota admission
before the database closes. Application catalog checks also pass four tests.

The full workspace passes `just fmt`, `just lint` and `just test`: 953 tests pass,
two external/opt-in tests remain ignored. A fresh all-feature workspace target is
7.35 GiB. Mixing focused/default-feature and workspace/all-feature passes had
accumulated 17.7 GiB and correctly failed the 16 GiB post-build guard. Cleaning
completed artifacts and rerunning the exact workspace recipe passed without
raising the limit.
