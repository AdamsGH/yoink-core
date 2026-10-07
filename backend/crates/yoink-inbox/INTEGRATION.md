# Inbox: completed module-local acceptance and coordinator wiring

This handoff supersedes the historical partial-slice gap lists. The preserved
initial 28-test slice and subsequent implementations remain in this worktree.
Module-local orchestration, storage, HTTP and deterministic source regressions
are accepted by the focused checks below. This is **not** a claim of a deployed
or end-to-end integrated native product: shared runtime, Insight, confined worker,
frontend, Telegram transport and global migrations remain coordinator-owned.

## 1. Schema registration (explicit; never HTTP startup)

After `yoink-storage/migrations-turso/0002_application.sql`, register these owned
proposals using the coordinator's next approved migration identities/checksums:

1. `sql/durable_jobs.sql` — `inbox_jobs`, repeatable read/completion work.
2. `sql/github_mutations.sql` — `inbox_gh_mutations`, external write intents.
3. `sql/notification_outbox.sql` — `inbox_notifications`, completion snapshots.
4. `sql/backfill_jobs.sql` — imported pending/enriched work admission; run in the
   same upgrade transaction, excluding archived items. It is replay-safe.

These add **three tables**, not three global revisions reserved by this module.
Native audit/lease/retry timestamps are INTEGER **microseconds**; public item and
catalogue timestamps are RFC3339. Existing IDs/bridges/metadata stay in place.
No credentials are migrated/copied. Category normalized_name is explicitly
written here because native storage does not have PostgreSQL's generated column.
Fixtures instantiate the real native application SQL plus these proposals; no
production DB or imported PostgreSQL artifact has been accessed. Backup/restore,
import round-trip, registered checksums/table-count and upgrade/rollback acceptance
are coordinator gates. Stop admission/workers before any rollback; do not simply
drop ledgers containing unknown external effects.

## 2. Server, authority and worker registration

- Add `yoink-inbox` to the central server and construct:
  `HttpState { store: Store::new(shared_db.clone()), auth, insight, github }`.
- Mount `http::router(state)` at `/api/v1/inbox` behind shared authentication.
  Insert `Extension(http::Actor(authenticated_user_id))` from middleware only;
  never from request fields. Middleware emits 401 for unauthenticated requests
  before entering this router (missing extension fails closed with 500).
- `contracts::Authorization::require(user,feature)` must re-read effective grants
  and bans. Features: `inbox:ingest`, `inbox:classify`, `inbox:gh_sync`,
  `inbox:gh_write`, `inbox:share`. `require_admin` implements legacy
  **moderator/admin/owner** access plus bans, despite its method name.
- Existing stable Insight calls remain `github_token(user,write)`,
  `complete(user,prompt)`, `language(user)`, returning `contracts::Call`.
  Resolve read/public_repo credentials at use time, not into jobs/DTOs/logs.
  Map adapter failures to stable `Error` variants, not raw provider bodies.
- Run `worker::Worker::tick(clock)` under central cancellation/shutdown ownership.
  Supply positive `lease_micros`, `retry_micros`, `max_attempts`, a microsecond
  clock and bounded polling. Schedule periodic authorized user `stars_sync`
  admission separately. Enrich/classify and star snapshots are lease-fenced.
  Classification checks both ingest and classify grants around completion;
  star/list pagination gets a final authority check before committing.
- **Never** put GitHub mutations or Telegram send attempts on this repeatable
  Worker retry path. No DB lock spans external content/Insight/GitHub work.

## 3. Confined content wiring and exact source route boundaries

`contracts::ContentProvider::extract(user,url) -> Content` remains stable. Either
implement it with the confined worker directly, or instantiate
`enrich::FallbackContent { stages, github, insight, max_chars }` per call and use
`extract_content(user,url)`. `Stages` provides:

- `fetch(user,url,headers)` — enforce shared SSRF/DNS/redirect/size/deadline/proxy
  rules. It must never obtain a GitHub credential or send one to a provider.
- `readability(html,url)` and `trafilatura(html)` — pinned worker primitives;
  Inbox owns ordering/thresholds, not Python process launching.
- `structured(Value)` — pure formatting, **no network**. Implement with pinned
  `toon_format.encode` for the legacy TOON representation. The default is the
  source's documented JSON fallback. `tests/fixtures/source.json` includes both
  original TOON and JSON representations for projected routes. JSON key order is
  not a semantic content guarantee. Full TOON byte matching belongs to the worker
  implementation of this encoding hook, not an invented native JSON replacement.

`github_content::{parse,fetch}` traces the preserved Insight fetch.py: repository
metadata+README; raw-before-contents file/base64 fallback; recursive filtered
trees; individual issues/PRs and their **different** comments APIs; open issue/PR
lists; commits (branch parameter); releases; API passthrough including users paths
and query strings. `Github::raw_file` is a new defaultable provider hook; the
provided `GithubClient` implements bounded reads at the fixed raw GitHub origin.
The production client fixes API/raw origins and refuses redirects; with_endpoint
is the loopback/gateway fixture hook, not a client-selectable HTTP destination.

Do not implement new profile/org/compare/commit/release-asset extraction policy:
source one-segment profile URLs have no route; `/orgs/a` is parsed as owner=orgs,
repo=a and ordinarily fails into providers; compare/single commit/unknown paths
fall back to **repo info**; release asset paths use the releases list. Ref parsing
uses the first segment, preserving source's branch-with-slashes ambiguity rather
than inventing resolution. `www.github.com` is not an Insight API host.

Non-GitHub policy remains llms.txt matching, readability (>200 characters),
trafilatura (>200), Jina then curl.md (chrome removal and >=100 useful characters),
raw HTML last; GitHub skips llms/readability/trafilatura/raw-shell extraction.
Original-page card fetch is separate from matched llms content. Hero/avatar/bio,
byline/published/frontmatter/footer stripping has an exact source regression.
The native raw HTML/card helper is a best-effort fallback, not a proof of identical
BeautifulSoup output on arbitrary malformed HTML; pinned parser/confinement
acceptance is a shared-worker gate. Existing metadata/manual assignments and
star AI fields are retained; bridge linking runs before/after star snapshots and
uses numeric repo identity when enrichment supplies it.

## 4. Recovery HTTP contract and frontend obligations

All paths below are relative to `/api/v1/inbox` and require fresh current write
access for remote recovery. No route returns tokens, send tokens, prepared
payloads or raw external error messages.

| Method/path | Contract |
|---|---|
| `POST /folders` | Defaults `is_local:true`. Explicit `is_local:false` requires `Idempotency-Key` **before admission**, plus write access/credential. Success 201 returns folder DTO. |
| `GET /gh_mutations` | 200 active `[{id,state,created_at}]`, user-scoped. |
| `GET /gh_mutations/{id}` | 200 `{id,state,folder_id,error_code,created_at,updated_at,can_cancel,can_inspect_candidates,operation_key_matches}`. `folder_id` exposed only when applied. Optional `Idempotency-Key` proves a match against this user's stored request key without returning it. Missing or different keys yield `operation_key_matches:false`. Unknown/other user's ID is 404. |
| `POST /gh_mutations/{id}/reconcile` | Prepared may send **once**; confirmed atomically applies; sending/unknown performs reads only. Success 200 `{id,state,folder_id}`; unresolved is 409. Never replay an unknown create. |
| `GET /gh_mutations/{id}/create-candidates` | 200 `[{id,name,slug,description}]`, including `[]`. Re-reads catalogue, excluding persisted pre-send baseline and requiring name/description match. Not evidence that this client caused any candidate. |
| `POST /gh_mutations/{id}/adopt-create` | Body `{remote_id:string}`; explicit user selection is revalidated using fresh authority, token and catalogue. 200 applied intent/folder ID. An already read-synced candidate is reused, retaining its manual fields/pins/memberships. |
| `POST /gh_mutations/{id}/cancel` | 204 **prepared only**. Sending/unknown cannot be cancelled or cleared; 409. |
| `POST /folders/{id}/rename-remote` | Explicit remote rename body including `name`; 200 intent result. Ordinary local PUT/PATCH does not use this endpoint. |
| `POST /folders/{id}/delete-remote` | Explicit remote deletion; 204 when applied. Ordinary local DELETE does not contact GitHub. |

Unresolved write admission/effect returns 409 with stable
`{"detail":"GitHub mutation pending reconciliation","mutation_id":"..."}`.
Other statuses: invalid request/key 422; denied authority/credential 403;
scoped missing resources 404; definite rate limit 429. Do not show raw payloads
or provider errors as UI diagnostics.

### Required remote-create key lifecycle

Frontend creates an opaque stable key (1..128 **bytes**) once per logical remote
create, persists it with the original request before POST, and reuses **the same
key and normalized payload** after response loss/reload. Keys are user-scoped.
A key used with another payload is 422; an applied key returns its existing
folder rather than contacting GitHub; pending keys remain pending. Do not rotate
keys, regenerate payload defaults or offer “retry with a new key” to bypass an
ambiguous result. Fetch active/status/candidates instead and require selection
before adoption. A conflict ID can identify another request. Clear a saved
rejected/cancelled create only after status proves its exact persisted key matches.
An applied status alone does not clear the saved request. Retrieve its folder
through an explicit POST with the same key and original body. GitHub itself does
**not** accept this Inbox idempotency key or provide list-create idempotency/CAS.

The mutation ledger has `prepared -> sending -> confirmed -> applied`, plus
`unknown` and definite `rejected`. Send CAS and one active intent per user provide
serialization across restart; local effects and applied state commit together.
Crash after remote acceptance but before response stays sending/unknown. Read
reconciliation may establish observable edit/delete/membership/star state, not
which client caused it. Read/merge/write memberships cannot defeat concurrent
third-party GitHub list changes; there is no remote CAS.

### Unknown create with no candidate: original actions, not new policy

`[]` is **not** proof of no effect and never permits resend/abandon/reset. Source
has no journal-reset policy; this module adds none. The original router permits:
local folder creation; local folder PUT/PATCH/delete (even on linked folders);
local category/manual assignment, archive/delete, hints/prompts and rules; reads
and authorized read sync; organizing into a local folder. These remain usable
without clearing the unknown intent. Its hidden reservation cannot be edited or
deleted through ordinary folder endpoints. Remote writes remain fenced while
unknown; performing GitHub UI actions out of band is not proof of which request
succeeded and does not unlock the ledger. An unobservable outcome requires
coordinator/product resolution, **not** an invented automatic retry policy.

## 5. Durable completion-to-notification delivery contract

Owned outbox admission is inside `Store::apply_classification`'s transaction,
after classification/rules/bridge projection and before job done. A bot completion
stores immutable title/URL/summary and the resolved classification category names
(the legacy notification inputs, not a destructive rewrite of manual bindings).
The completion key is job ID + lease token; stale/repeated completion cannot
admit a second event. Web/API items and archived/failed outcomes do not notify.

`telegram::bind_reply(store,auth,user,item,ReplyTarget)` must be called **only**
after the shared initial-save send ledger confirms the actual bot receipt.
Never bind from the incoming user's message, an intended address or an ambiguous
send. First confirmed chat/message wins; same binding is replay-safe; conflicting
or wrong-user binding returns false. Binding and waiting-event release commit
in one transaction. Classification may finish before the receipt: event remains
`waiting` until confirmed binding, so no process-local callback can lose it.

Coordinator dispatcher algorithm (all times microseconds):

1. `claim_notification(now,lease)` transactionally cancels inactive pre-send work,
   quarantines expired sending as unknown and claims ready/expired **pre-send**
   leases with a new token. It serializes edits of the same confirmed reply in
   durable insertion order; older sending/unknown events block newer ones.
2. Render the immutable payload using the existing transport renderer. Legacy
   plain body is summary, then category tags (`spaces -> '_'`) separated by blank
   lines, or title/URL if empty. Bound chat/message IDs are internal only.
3. `begin_notification_send(auth,route,&claim,clock)` refreshes ingest grant/bans,
   calls `notifications::RouteAuthority::require(user,ReplyTarget)` for current
   private/group/topic/destination policy, refreshes user authority again, and
   CASes a live claim to sending while checking current item and exact confirmed
   target. Returned **send_token** is the shared delivery ledger identity. If
   authority/CAS fails, do not send; a pre-send claim can expire or be retried.
4. Shared delivery ledger must durably reserve/lookup **this same send_token**,
   bound to user/chat/message/payload digest, then CAS at-most-once dispatch.
   No separate sender may ignore that ledger. Call the actual Bot API **edit**
   only after successful admission and fresh route policy at its own boundary.
5. Record the trusted result with
   `notification_outcome(user,id,send_token,Outcome,now)`:
   - `Confirmed { receipt }`: 1..128-byte opaque durable confirmed-receipt ID;
     atomically ack, including after lease expiry/restart/archive/delete.
   - `Unknown`: quarantine; never ordinary retry. Timeouts, resets, 5xx, missing
     receipts and process death are unknown, not definitely-not-sent.
   - `DefinitelyNotSent { retry_at }`: only an authoritative **sealed/fenced**
     shared ledger result proving no attempt occurred; returns ready for a fresh
     send boundary/authority check. Absence of a ledger row alone is insufficient
     unless late dispatch under that token has been atomically prevented.
6. On restart use `unknown_notifications(user)` (sending + unknown) and the
   persisted send_token to look up that shared ledger. Record confirmed/not-sent
   proof or retain unknown. Never blindly invoke `Telegram::edit` again.

`retry_notification(&claim,now,available_at)` is only a live **pre-send** lease CAS.
It cannot transition sending/unknown. All outcome updates are user/send-token
fenced; old results cannot ack a newer attempt. Recording an already confirmed
effect does not require a new user grant because it initiates no effect. Return
bool indicates whether the fenced update changed a row; duplicate/stale callbacks
are harmless false, not authorization to resend. No outbox recovery HTTP route is
exposed to end users: delivery proofs are coordinator-internal authority.

The existing `contracts::Telegram::edit -> Call<()>` alone has no confirmed/unknown
receipt semantics. Do **not** use it as the outbox dispatcher without the above
shared delivery ledger. Shared central rendering, streamed save, typing, callbacks,
identity bootstrap and route/send-ledger implementation remain integration work.

## 6. Verification and remaining shared acceptance gates

From backend (single reused target, Cargo jobs=1):
`just format-inbox`, `just check-inbox`, `just test-crate yoink-inbox`.
From this crate: `just source-oracle`, `just postgres-oracle`.

Latest focused Rust result: **63 tests passed, 0 failed**, doc tests 0. Source
oracle executes preserved schema/fetch files read-only using the already-built
`yoink/yoink:latest` image, network disabled; 23 route cases, 33 schema cases and
provider stripping fixture. PostgreSQL oracle runs an isolated no-network,
no-port, tmpfs-only `postgres:16-alpine` DB: **16 C.UTF-8 ILIKE cases pass**. Source
fixture generation is explicitly `just source-oracle-record`; normal checks only
compare. No host Python, image build/pull, live token, external writes, staging or
commit was used. Resource preflight is included in typed check/oracle recipes.

Still coordinator-owned/not claimed tested here: real effective grants/bans and
transport identity; central server/worker scheduling/shutdown; Insight/BYOK and
confined extractor/TOON primitives; frontend key/recovery UI; shared confirmed
initial send and notification-delivery ledgers; localized/streamed Telegram UX;
global migration/import/rollback acceptance. ILIKE is proved against C.UTF-8
simple lowercase, not every possible ICU/Turkic locale: preserve/confirm the
source deployment collation before claiming exact imported search parity.
Search evaluates native scoped rows before pagination to avoid SQLite ASCII-only
LIKE; operational budgets for very large user catalogues need shared acceptance.
