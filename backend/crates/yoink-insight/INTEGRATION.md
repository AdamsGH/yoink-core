# Coordinator-owned integration contracts

The patch is intentionally separate from module code. The workspace glob already
finds this crate; the lock delta contains only its package entry.

## Runtime construction

After native schema validation/import and shared auth/identity installation:

```rust
let provider = Arc::new(yoink_insight::provider::HttpCompletion::new(
    Duration::from_secs(120), 1024 * 1024,
)?);
let research = Arc::new(yoink_insight::research::ResponsesResearch::new(
    Duration::from_secs(90), 1024 * 1024,
)?);
let insight = yoink_insight::Insight::new(
    database.clone(), insight_config, confined_content_source, provider,
)?.with_research(research);
let oauth = yoink_insight::oauth::DeviceOAuth::new(
    insight.clone(), "Iv1.b507a08c87ecfe98".into(),
    configured_public_repo_client_id, shutdown.child_token(),
)?;
let http = yoink_insight::http::HttpState {
    insight: insight.clone(), oauth: oauth.clone(), shutdown: shutdown.clone(),
};
```

`confined_content_source: Arc<dyn ContentSource>` is a required non-optional
integration dependency, not a no-op implementation. In this worktree it cannot
be constructed from `yoink-executor` because content-fetch has no wire variant.

Map the shared config loader to `Config` (do not add a parallel environment/auth
loader): owner ID, Gemini key/model, transcript languages, gateway URL/key,
OpenRouter URL/key/mode, TLDR default model and search model, independent daily
limits, max characters and deadline. Native bot_settings override persisted route
settings on every request. Existing default URL already includes `/v1`; do not
append another `/v1`. Keys never enter HTTP config responses.

On shutdown, cancel the runtime token and `oauth.shutdown().await` before closing
the DB. Shared content worker must join its own child process cancellation.

## HTTP

Apply the bridge hunk and nest beneath `/api/v1/insight`. Bridge authenticates with
existing `AuthState::service` + `authenticated_id`; Insight reauthorizes every
operation using current DB state. Do not create a separate JWT secret or pass a
frontend role. API-key authentication, if enabled for these routes by coordinator,
should use the same shared principal validation and insert the same extension.

## Telegram

Add `Option<Insight>` to the shared Telegram adapter state and intercept only
`about`, `summary`, and `tldr` after existing authenticated private/group admission.
Pass the observed user ID, full command text, and a child cancellation token to
`telegram::execute`. Use existing language/localized headers and stable error
mapping; send its result via shared destination delivery policy in plain-text
chunks from `telegram::messages`. Do not bypass group policy or send to an ID from
command arguments. Existing commands/owner UI still own `/access` and settings
menus; bind them to `Insight::{grant,revoke,settings,update_settings}`. Refresh
shared menus after access/BYOK/config updates (coordinator-owned invalidation).

## Activity

Register `Insight::activity(user_id)` with the shared activity aggregation adapter.
For administrator viewing another user's activity, the aggregator must authorize
the administrator before invoking its internal provider path; the public module
method takes the subject and also denies globally blocked identities.

## Shared identity future consolidation

The current module reuses `yoink_access::effective_grant` and shared registered
features, with current held-connection native reads. `sql/active_tags.sql` is an
exact read-only copy of current identity membership SQL to preserve transaction
fencing. Prefer coordinator exposing the shared identity's native connection-bound
snapshot/authorization API later so all products consume one membership reader;
do not switch to a stale snapshot taken before a write transaction.

## Concrete Inbox contract adapter (now implemented)

`src/inbox.rs` implements DB-backed `github_token`, `complete` and `language`;
no unimplemented callbacks or unavailable Insight methods are involved. In the
coordinator's crate that depends on both products, instantiate the exact adapter:

```rust
yoink_insight::impl_inbox_adapter!(pub RuntimeInboxInsight);
let inbox_insight = RuntimeInboxInsight::new(insight.clone(), shutdown.clone());
// &inbox_insight coerces directly to &dyn yoink_inbox::contracts::Insight
// for Inbox pipeline::execute or worker::Worker.insight.
```

This macro emits the full trait implementation and caller-local wrapper (Rust's
orphan rule); it avoids cyclic dependencies and cross-worktree Cargo paths. Regression tests now compile this macro against the **actual native yoink-inbox crate**,
not the old copied contract/error fixture. This is dependency acceptance only,
not full Insight product runtime/router/Telegram acceptance.

### Inbox-only construction (available before content integration)

```rust
let completion = Arc::new(yoink_insight::provider::HttpCompletion::new(
    Duration::from_secs(120), 1024 * 1024,
)?);
let insight = yoink_insight::Insight::completion_only(database.clone(), config, completion)?;
yoink_insight::impl_inbox_adapter!(pub RuntimeInboxInsight);
let inbox_insight = RuntimeInboxInsight::new(insight.clone(), shutdown.clone());
// implements yoink_inbox::contracts::Insight directly
// Later: insight.with_content(real_confined_content_source)
```

This is the same Insight service/route/credential implementation, not another LLM
service. No dummy ContentSource is required. Full `Insight::new` remains available;
completion-only instances explicitly return content_unavailable for content use.
Coordinator owns shared Config mapping and runtime/router composition. Use Config's
owner_id, gateway_base/key, openrouter_base/key, mode (auto/a2a/openrouter), model,
request_timeout; other Config fields retain their existing full-product meaning.
Native bot_settings override routes on every request. Gateway base already includes
/v1. HttpCompletion and service both enforce deadlines/cancellation; output bytes
and prompt characters are bounded. Custom BYOK uses existing public-DNS pinning,
no inherited proxy and no redirects; trusted operator gateways permit local HTTP.

* Read token requires current effective inbox:ingest OR inbox:gh_sync. Shared ingest
  is role-granted to User by default; deleting an explicit grant does not revoke a
  still-effective shared role grant. Restricted identities require explicit grants.
  Current bans/expiry are enforced; write token requires inbox:gh_write and never
  substitutes the read credential for absent upgraded public_repo credentials.
* Completion requires inbox:classify or inbox:gh_organise, with fresh route and grant
  checks after I/O. Main pipeline separately checks inbox:classify before/after.
* Language exactly follows legacy InsightAccess.lang, default en. Insight settings
  and shared UI language do not override it.
* Cancelled maps to Inbox Cancelled (HTTP 503, worker cancellation status/retry),
  never Forbidden. Denial/current authorization changes remain Forbidden.
* Only provider_rate_limited (429), provider_unavailable (transport/5xx), timeout
  and native DB failures map to retryable RateLimited/External. Auth, quota,
  credits, model/config, parse/size/protocol and other completion failures are
  terminal, sanitized Inbox Completion codes (HTTP 422). Invalid prompt is Invalid.
  Worker fail_job reporting accepts only the explicit sanitized status whitelist;
  no raw provider messages or credentials enter responses/statuses.

The later full Insight product remains pending. Router integration stays deferred
until coordinator constructs all real dependencies. Tokens already returned to a
caller cannot be revoked in-place: callers must reauthorize/refetch per attempt.

## Incremental streaming handoff (resumed work)

Use `telegram::execute_stream(&insight, observed_uid, full_command, &sink,
shutdown.child_token()).await`, rather than splitting the output of `execute`.
Implement the object-safe `provider::DeltaSink::delta(&self, text: &str) ->
FutureResult<'_, ()>` with coordinator-owned Telegram draft/send/edit logic.
Each callback is awaited, applies backpressure, and contains a **delta**, not the
full transcript. Accumulate and throttle edits in the bridge; model text is plain
untrusted text. Do not hold a native DB connection across Telegram I/O.

The returned `Summary` is final only after fresh authorization/route/research
rechecks and cache/usage commit. On error, mark/remove the partial draft; never
present it as a successful answer. Cached `Summary` returns without sink calls:
render it once. An unsupported injected Completion returns
`streaming_unavailable`, never buffered fake deltas. Real HttpCompletion consumes
OpenAI, Anthropic and Gemini SSE; it only falls back before any sink invocation
and never for BYOK. Sink failure, cancellation, malformed frames, absent terminal,
upstream error and wire-byte overflow abort generation.

The ContentSource.fetch signature and file have not changed in resumed work;
content.rs SHA256 at resumption handoff:
`ead84d9982f9d1fe3833e32d0365733ae523154814de3caee2960b37b7ddc9c8`.
The separate content worker owns `worker_content.rs`; no such file was edited or
created here. It must be supplied by that owner before integrated fresh-content
acceptance. Inbox macro/adapter and its fresh read/write authority are unchanged.

### Exact legacy consumer findings

* `commands/_runner.py` consumes `gemini.stream_command`, which calls SDK
  `aio.models.generate_content_stream(contents=prompt)` with **no grounding
  tools**. `commands/tldr.py` consumes `services/tldr.py::stream_llm`; that
  executes `services/byok.py::stream_chat` and only retries before text emission.
* `services/byok.py::_fetch_openai_models/_fetch_anthropic_models` and
  `api/router.py::_fetch_gateway_models` read one `data` array. Pagination added
  here is bounded protocol consumption, not a claim that legacy iterated pages.
  Gemini catalogues filter `supportedGenerationMethods=generateContent`; search
  capability is not fabricated from Gemini names. BYOK's executable search-name
  patterns are sonar, perplexity/, :online, -online, -search-preview.
* `services/tldr.py::_fetch_research` executes `search_client.py::search_fetch`
  via independent cited Responses research. No Gemini grounding invocation was
  found in the pinned plugin. Existing research wire tests enforce citations.
* `frontend/src/pages/settings/GithubLoginDialog.tsx` reads `next.username` for
  success toast and `onSuccess`; `GithubWriteCard.tsx` uses it for write-login
  toast. Both flow statuses now populate that field best-effort. `ByokCard.tsx`
  reads `supports_websearch` for selection/warnings; that UI annotation alone is
  not evidence that any selected provider executes search tools.

## Completed management contracts (final module-local acceptance)

The previously documented disabled-owner-BYOK and live-model-query gaps are closed.
No shared router/frontend changes are required for these paths: `http::router`
now deserializes `ModelProbeOptions { url, api_key }` from the existing queries.
All identities still come exclusively from `AuthenticatedUser`; owner/admin
checks are current native DB policy, including configured-owner identity and bans.

* `PUT /byok/me`, `POST /byok/me/test`, `POST /byok/me/refresh-models`: current
  owners may operate when global BYOK is disabled. Admin alone is not an owner
  exception. Normal users still require the enabled toggle. GET reports the real
  disabled toggle, never a fabricated true value. Delete remains available to a
  nonblocked subject regardless of toggle.
* PUT trims key/model/base input and reuses a stored key if the supplied key is
  missing/blank, including when provider/base selection changes, as the original
  router does. It probes **before** writing, then reauthorizes and compares the
  full stored credential revision before saving. Canceled, revoked, deleted or
  superseded work cannot overwrite/resurrect credentials. Failure preserves prior
  catalogue/timestamps for the UI but stores sanitized test_error, which prohibits
  a BYOK provider grant. Initial failure has null tested_at. Refresh also preserves
  prior successful metadata on failure. Short keys are masked as `***`.
* Input-only test with absent key reuses the stored key and fences its revision;
  absent stored config returns `{ok:false,error:"api_key_required",models:[]}`.
  Explicit-key tests never persist a credential or grant. Missing saved config
  on refresh is 404 (`not_found`). BYOK-disabled mutation/test/refresh is 403.
* `/models` owner live overrides are ephemeral. Nonowner overrides (including
  malformed ones) are ignored and only configured IDs are returned. Provider
  failure returns `[{id: Config.model}]`, matching pinned `_fetch_gateway_models`.
* `/config/test` is admin/owner, and its success response is exactly
  `{ok:true,model_count:N,url:chosen_base}`. Failure is
  `{ok:false,error:"provider_probe_failed",url:chosen_base}` with no raw provider
  body or credential. This matches the frontend's required shape while redacting
  legacy raw exception text.
* For both live paths: URL override with no key uses anonymous access, NOT the
  stored key. A key-only query is ignored and uses persisted URL/key. Trailing
  slashes are removed and `/v1` appended only if absent. Override input must be a
  bounded HTTP(S) base without URL userinfo, query or fragment; keys are separate
  headers. Trusted operator gateways may be internal HTTP endpoints; custom BYOK
  retains public-DNS pinning and no redirects. Neither override persists settings.
* Post-provider checks reject current bans/demotion and persisted-route changes.
  All provider management calls have cancellation and a total configured deadline.
  Axum save now uses the same shutdown/disconnect cancellation guard as probes.

Public reusable methods: existing `save_byok`, `models`, `test_gateway` remain;
new `save_byok_with_cancel`, `models_with_options`, `test_gateway_with_options`
allow coordinator cancellation/options without bypassing module authority.
`ModelProbeOptions` is exported from the crate root and intentionally has no Debug
because it can contain an API key.

Pinned execution/consumers: `api/router.py::_is_owner`, `put_my_byok`,
`test_my_byok`, `refresh_my_byok_models`, `list_tldr_models`, `test_gateway`,
`_fetch_gateway_models`; frontend `api/insight.ts::{listModels,testGateway}`,
`pages/admin/TldrConfigPage.tsx::{testConnection,refreshModels}`, and
`pages/settings/ByokCard.tsx` save/test/refresh. These are implemented contracts,
not declaration-only UI features.

Cross-module hooks remain the previously listed shared identity insertion,
router nesting, Config mapping, two-table migration review, activity registration,
Telegram execute_stream sink/delivery/menu invalidation, OAuth shutdown joining,
ContentSource worker installation, and exact Inbox adapter instantiation. Refresh
menus after BYOK saves/deletes/refreshes and global-toggle mutations. No change to
ContentSource.fetch, OAuth expiry/generation, streaming lifecycle or Inbox token
contracts is necessary to consume the completed management paths.
