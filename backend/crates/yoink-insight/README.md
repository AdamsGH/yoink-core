# Native Insight

Product-owned Rust use cases, Axum routes and Telegram command adapter. Original
`plugins/yoink-insight` snapshots were read only and remain untouched. No UI was
reduced or rewritten.

## Traced contracts and implemented surfaces

* `commands/{about,summary,tldr,_runner}.py`, `services/{gemini,tldr}.py`:
  exact YouTube host/video validation, transcript-only about/summary, the original
  instruction defaults (copied as data), builtin aliases, custom aliases, domain
  wildcard bindings, mandatory focus and language/custom-prompt selection.
* `services/{llm,byok}.py`, API BYOK/config/models routes: gateway `/chat/completions`,
  OpenRouter mode/fallback, current-DB tested BYOK, OpenAI-compatible, Anthropic
  `/messages` and Gemini `:generateContent` payload/response contracts. Async
  catalogue/probes, redacted config reads and credential replacement invalidation.
* `services/search_client.py`: an actual bounded Responses web-search adapter,
  citation extraction and separately labelled research. Source-under-review URLs
  are excluded from corroborating sources. No citations means research failure,
  not fabricated verification. `with_research` installs it; `ContentSource::research`
  can instead delegate to an existing shared independent-research adapter.
* `services/{access,byok_access}.py`, native shared identity: shared role/spec/grant
  policy, native current bans/expiry, current BYOK toggle (including imported
  truthy forms), effective current membership-tag contributions/suppressions.
  Access management updates both unified permissions and the legacy response view.
* Native settings/prompts/aliases, usage and activity, frontend statistics shape,
  model/admin/BYOK management, per-user private prompt/model/content/research cache,
  default imported YouTube cache reads, independent daily quota reservations.
* `services/github_device.py` and GitHub router endpoints: async cancellable read
  and public_repo device flows, replacement/deletion fencing, slow_down, expiry,
  current-DB permission checks before credential persistence, scope validation,
  bounded response size, shutdown task joining. No star/unstar or token revocation
  external mutations are issued. OAuth status never exposes device/access tokens.

`http::router` uses relative paths; mount at `/api/v1/insight`. The shared frontend
settings, alias, BYOK, access, config, model, statistics and GitHub paths are present.
`POST /summarize` is an additional reusable buffered HTTP use case. Shared auth
must insert `AuthenticatedUser`; absence is 401. JWT roles are never authority.

`telegram::execute` is a transport-independent Telegram command adapter for
`/about`, `/summary`, `/tldr`, including `@bot`, aliases and focus. The shared Bot
API adapter still owns dispatch, menu refresh, destination policy, localization
and actual sendMessage. `telegram::messages` emits UTF-8-safe plain-text chunks;
do not apply HTML parse mode to model output.

## Reuse from Inbox

`Insight::complete` shares current-DB routing, provider contracts, bounded I/O and
post-I/O credential/permission rechecks. `Insight::summarize` shares the complete
content/prompt/research/quota/cache use case. `ContentSource`, `FetchOptions`,
`Content`, `Citation`, `Completion`, and `Research` are reusable object-safe async
interfaces. Use cancellation children of the runtime shutdown token.

## Required coordinator integrations (not silently implemented)

See `integration.patch` for minimal dependency/auth/router registration hunks.
They are proposals, not applied changes to other crates. `INTEGRATION.md` specifies
runtime and Telegram construction.

**Content-worker blocker:** this worktree's shared executor/protocol has media and
search requests, but no content-fetch/transcript request or response. A real
`ContentSource` implementation must be supplied by the confined content-fetch
worker owner. This crate deliberately does not fetch arbitrary content URLs in
Rust or run host Python. `FetchOptions` supplies ordered languages (`en,ru`),
transcript-only flag, configurable character cap, and a redacted read token only
for exact GitHub hosts. Worker implementation must also confine DNS/redirects and
never forward a token to a different origin. Without that integration, fresh
summaries cannot be served by runtime; passing fixture tests is not an end-to-end
content-worker acceptance claim.

Actual incremental completion is implemented via `Completion::stream`,
`provider::DeltaSink`, `Insight::summarize_stream` and `telegram::execute_stream`.
OpenAI/Anthropic/Gemini SSE is consumed from network chunks, with awaited delivery,
wire-byte limits, terminal validation, cancellation, per-delta current-DB checks
and final accounting/rechecks. Partial delivery prevents fallback; BYOK never
falls back. Cached results emit no artificial deltas. Buffered HTTP remains an
explicit separate API, not a claim of streaming.

Catalogue consumption follows bounded Gemini page tokens and Anthropic cursors;
OpenAI-compatible catalogues remain single-page (no invented next-URL protocol).
Gemini embedding-only models are excluded. Model-name search annotations preserve
the executable legacy patterns, but Gemini names are NOT grounding evidence:
legacy `gemini.py` streams plain `generate_content_stream` without search tools,
BYOK Gemini uses OpenAI compatibility, and TLDR independent research executes
`search_client.py` Responses. This module adds no fictional Gemini research path.
OAuth read/write success now does bounded best-effort `/user` lookup, returns
`username` to the existing dialogs, and rechecks generation/expiry/current DB
before saving; failed lookup still permits authorized token persistence.

The documented module-local management gaps are now closed. Disabled BYOK permits
current DB/configured owners to save, test and refresh without enabling BYOK for
other users or bypassing bans. PUT probes before persistence, honors stored-key
reuse and trimmed inputs, fences credential revisions, and preserves legacy
catalogue/timestamps on failed replacement while test_error denies provider access.

`GET /models?url=...&api_key=...` honors owner overrides; nonowners receive only
the configured allowed IDs and their overrides are ignored. `GET /config/test`
honors admin/owner overrides and returns the frontend's `model_count`/`url` shape.
Overrides are never persisted. Omitted override key means anonymous; key-only
query uses stored settings. `/v1` normalization is applied once for these probes.
Provider errors retain legacy model-list fallback and sanitized test failures;
current role/ban/route checks fence results after I/O. Secrets never enter URLs
returned by the API or upstream error bodies. BYOK endpoints remain DNS-confined,
while authenticated operator gateway overrides intentionally support internal
HTTP gateways, as the pinned admin UI requires.

Remaining acceptance is cross-module: shared Bot API dispatch/menu/localized error
binding, actual draft edits, runtime/router/migrations and Inbox/content wiring.
Dynamic live-provider behavior is unverified and not inferred from catalogue facts.

## Schema plan (no global migration number claimed)

`sql/native.sql` adds two module-owned STRICT tables and indexes. `models.rs`
contains matching cache/reservation models. Keep all imported Insight tables and
microsecond timestamp storage unchanged.

Coordinator should:

1. Assign ordering after its latest reviewed native migration; apply this SQL once
   through the existing schema runner, transactionally. No startup `CREATE IF NOT
   EXISTS` or global migration number is invented here.
2. Update schema/table expectations by **two tables**, the native source manifest
   and schema verifier; include indexes and foreign-key assertions. Existing
   legacy import mapping requires no column conversion or renumbering.
3. Test upgraded/imported native DBs, preserving settings, BYOK credentials,
   prompts, usage, aliases, access, and default Gemini cache values.
4. Do not backfill shared TLDR cache into per-user native cache: old cache keys do
   not prove prompt/model/user/research identity. Imported default about/summary
   entries are still read without requiring a live Gemini credential.

Successful cache writes and successful usage accounting are committed together
in a native RAII transaction on the held single connection. Management writes
also use native RAII transactions. Atomic admission is serialized on that same
shared connection.
Failures release their reservations; a dropped/timeout request can leave a bounded
lease until request deadline +5s, reclaimed on next admission. No external
provider work survives the cancellation scope.

## Checks

Use only the isolated `backend/target`, local jobs=1:

```
cd backend
just test-insight
just check-insight
```

The tests use disposable private native DBs, fixture completions and loopback HTTP
providers/OAuth servers. No live credentials, external mutations, Python host
execution, Docker builds, full workspace checks or commits are needed.

### Concrete Inbox adapter

`inbox::InboxAdapter` and `impl_inbox_adapter!` implement the supplied Inbox trait,
including current read/write-token grants, separate credential storage, current
language (including imported AI preference), classification/organise completion
routing, cancellation and stable redacted Inbox error mapping. See the concrete
instantiation and exact contract checksum in `INTEGRATION.md`. Five native adapter
regressions cover real DB-backed behavior; these are not wiring-note-only methods.
