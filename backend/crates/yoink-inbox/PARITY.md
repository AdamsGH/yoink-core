# Module-local acceptance: executable source trace

Sources below are preserved read-only `plugins/yoink-inbox` files unless noted.
Executable router/schemas/services/frontend clients outrank README descriptions.
The owned contracts and focused tests are complete; central product integration
is separately enumerated in [INTEGRATION.md](INTEGRATION.md), not hidden as an
unimplemented next module chunk.

| Behavior / source | Owned implementation and deterministic evidence |
|---|---|
| URL/dedup/admission: `services/ingest.py`, `gh_detect.py`, item router/schema | `normalize.rs`, `Store::ingest`, `http.rs`; per-user identity, normalized URLs, rules + job in one transaction, HTTP default source=api and HTTP(S) URL validation, native/HTTP regressions |
| Cards and content: `services/enrich.py`; Insight `services/fetch.py` | `enrich.rs`, `github_content.rs`, `Store::apply_content`; card seed/original-page fetch separate from body, GitHub numeric identity, metadata preservation, fallback stage tests |
| GitHub API extraction routes: Insight `_parse_github_url` / `_github_fetch` | `github_content::{parse,fetch}`, bounded fixed-origin `GithubClient` raw/API reads; source oracle fixture covers repo+README, unknown compare/commit paths, release assets, raw-before-base64 contents, tree/prefix, issue/PR/comments, issue/PR lists, commits/branch, releases, users API query passthrough and source no-route cases |
| Markdown fallback decoration: Insight `_strip_provider_chrome` | Native frontmatter/hero/avatar/byline/published/bio/footer substitutions; exact original-source output fixture. Readability/trafilatura thresholds, provider useful-text test, raw/GitHub-shell refusal, cap ordering are orchestrated here |
| Categories/manual/shared: classify, models, assignment/category router, frontend `api/items.ts` | `catalog.rs`, `Store::assign_categories`, `apply_classification`; scoped parent/team references, manual attachments preserved, owner-only mutations, shared visibility and atomic assignment regressions |
| Archive/delete/reclassify: item router and frontend item client | Scoped archive/hard delete, late-job fences, classify grant refresh; jobs/bridges cascade, pre-send notifications cancel, already-sent late receipts still recorded |
| Stars/sync: `gh_stars.py`, `jobs.py`, frontend stars hooks | `github.rs`, `stars.rs`, `pipeline.rs`; scopes, ETags, safe-origin page links, malformed snapshot refusal, AI retention, removals and isolated snapshot tests |
| Lists/folders/pins/order: `gh_lists.py`, folder router/schema, folder frontend | `lists.rs`, `catalog.rs`, local full PUT versus partial PATCH, sync retains pins/order/manual membership. Source PUT/PATCH/DELETE stay local on linked folders; tested without any GitHub credential |
| External writes: `gh_write.py`, `gh_lists.py`; no original crash ledger | `mutations.rs`, owned SQL; paginated GraphQL memberships/catalog/items, durable prepare/send/confirmed/apply CAS, fresh credentials, keyed applied-response replay; ambiguous/crash/restart fixtures, explicit adoption of already read-synced candidates preserves manual metadata |
| Unknown create/no candidate | Explicit fixture proves empty candidates do not enable cancel/reconcile replay/new write; original local folder creation still works. No unknown-reset policy is invented |
| Rules: `services/rules.py`, frontend `api/rules.ts` | `rules.rs`, `Store::{test_rule,sweep_rule}`, `bridge::star_rules`; source arbitrary string triggers remain accepted (inert if unknown); full PUT defaults versus PATCH retention; star language/null/scoped archive cases |
| Item-star bridge: `gh_detect.py`, bridge/classify/star intent | `bridge.rs`; repository identity and numeric IDs, linking before/after sync, NULL-only AI seeding, preserve labels/manual fields/pins |
| Settings/prompts/admin: schemas/router/frontend rules client | User hint 4000-character and system prompt 8000-character limits, fresh bans and shared moderator/admin/owner threshold contract, Insight language/completion hooks |
| Save/inbox/stars: commands and plugin policies | `telegram.rs` command application facades, durable admission, confirmed-reply first-binding hook; shared dispatcher must provide entity/text-link extraction and transport UX |
| Completion notification: `services/classify.py:_notify_classified` | `notifications.rs`, `sql/notification_outbox.sql`, classification transaction; immutable title/URL/summary/resolved-category payload, receipt-before/after-completion binding, CAS claim/send/ack/retry, unknown send quarantine, serialization per reply, rollback/restart/ban/route/archive/delete tests |
| Identity/grants/token nonexposure: `_require_feature`, plugin features/command access | Injected fresh `Authorization`, authenticated `Actor` only, scoped SQL, auth checks around external calls, no credentials/Telegram targets in public item DTOs; real shared resolver remains coordinator gate |

## Completed source-validation/search/slug regressions

`tests/source_oracle.py` executes the **actual preserved** Pydantic schemas and
Insight parser/extractor inside an existing read-only, no-network Docker image.
It uses httpx mock responses, not production endpoints, and has no service/config
bootstrap. `source.json` records **23 URL cases, 33 schema cases**, API/raw response
fixtures, original TOON and JSON fallback content and provider decoration output.
Rust `source_parity.rs` consumes the same fixture: known fields, ignored extras,
integer/float-zero-string/bool coercion, Unicode length bounds, required/default
values, slug validators and folder null-PATCH semantics. HTTP tests execute
local PUT/PATCH/DELETE, full rule replacement, hint/admin limits and mandatory
remote-create key admission. Pydantic comments are not treated as enum validation.

`search.rs` implements PostgreSQL wildcard `%`, one-codepoint `_`, backslash
escape and Unicode **simple** lowercase before pagination. Scoped item search
uses title/summary/URL; stars uses full_name/description. This avoids SQLite's
ASCII-only lower/LIKE. Keyset and sorted offset fixtures verify filtering before
limits. Empty optional filters follow source truthiness, search is 2..128
characters, stars limit is source 1..100 (legacy frontend sometimes asks for 200;
coordinator must fix that client rather than claim source API accepts it).

`just postgres-oracle` runs the real PostgreSQL 16 C.UTF-8 operator for the same
16 cases used by Rust tests: Cyrillic, accents, simple sigma versus contextual
lowercase, dotted I, no sharp-s/ligature casefold expansion, escaped wildcards,
Unicode `_`, newlines and pagination-adjacent patterns. Other ICU/Turkic locales
are **not** claimed equivalent; confirm deployment collation at import/cutover.

Catalogue/team slug derivation is source ASCII `[a-z0-9]` lower/hyphen collapse,
64-character truncation and category/folder/team fallback, not transliteration.
Unique catalogue/team candidates are base then `-2..-19`, truncated before the
source `-x` fallback; lists sync has its separate original suffix sequence. Owner
isolation and collisions are tested. A pathological 64-character collision makes
the source fallback 66 characters and cannot persist in its 64-character column:
native returns stable 422 rather than inventing a new suffix algorithm.

## Deliberate fail-closed normalization (not false byte-for-byte claims)

- Source optional rule updates may accept null names/enabled/priority/trigger,
  then violate NOT NULL during persistence. Native rejects these before SQL,
  not a silent default or 500. Native also prevalidates persistence bounds
  (32-character icon, 16-character color, 24-character trigger, int32 order).
- Source remote-create errors were swallowed into local success. Native retains
  an unknown ledger and returns 409; only definitely rejected sends release
  admission. The required create key/recovery endpoints are safety contracts
  added here, not claims that original GitHub supplies idempotency.
- Source never had completion admission, crash/send uncertainty or receipt-proof
  machinery. The outbox intentionally suppresses archived/failed outcomes and
  never silently retries ambiguous edits. Central send-ledger proofs are required.
- Structured GitHub projections are proven against original data/headers/body
  content. Native's default encoder is source's supported JSON fallback;
  actual TOON bytes require the pinned `Stages::structured` worker primitive.
  Ref parsing preserves source ambiguity, not arbitrary slash-branch resolution.
- Generic card/raw HTML helpers are best-effort; tests prove fallback orchestration,
  not arbitrary malformed-HTML equivalence to pinned BeautifulSoup or real SSRF,
  proxy, DNS/deadline confinement. Those parser/transport primitives are shared
  adapter obligations, with exact hooks in INTEGRATION.md.
- Read reconciliation proves current observable state, not causal attribution;
  GitHub has no membership CAS against other applications.

## Verification boundary

Latest focused Rust run: **19 unit + 7 enrichment + 5 HTTP + 12 mutation recovery
+ 10 native + 5 notification + 5 source parity = 63 passing**, zero failures,
zero doc tests. `just check-inbox` denies Clippy warnings. Source oracle matches;
16 PostgreSQL ILIKE cases pass. Original 28-test slice is retained.

No shared runtime/Insight/content/frontend/global migration file was edited in
this continuation. Actual central wiring, production/import schema acceptance,
real user bootstrap/grants/bans, confinement/TOON, frontend recovery keys and
streamed/localized Telegram rendering/send ledgers remain coordinator integration
acceptance. These focused results are full **module-local** acceptance, not a
false claim that the untouched legacy frontend is ready for native cutover.
