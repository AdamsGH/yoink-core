# Native Inbox Telegram transport handoff

## Composition root contract

Inbox worker/read/HTTP services do **not** depend on Telegram. Only construct the
notification runtime when an actual Telegram adapter has passed `prepare/getMe`.
The composition root owns configuration, router installation, startup deadlines,
task ownership, readiness and shutdown. No new adapter spawns detached tasks.

After migrating the owning storage database (through migration **23**):

```rust
use std::time::Duration;
use yoink_telegram::{InboxRuntime, InboxRuntimeConfig};

let text = adapter.text_delivery(storage.clone(), Duration::from_secs(10))?;
let inbox_telegram = InboxRuntime::new(
    inbox_store.clone(), identities.clone(), text, groups_enabled,
).with_config(InboxRuntimeConfig {
    notification_lease: Duration::from_secs(120),
    recovery_batch: 64,
    dispatch_batch: 1,
})?;
let mut adapter = adapter.with_identity_store(identities.clone())
    .with_inbox(inbox_telegram.clone());
// Install before initialize_menus; the existing startup deadline bounds menus.
adapter.initialize_menus().await;
// Own both adapter.run(...) and inbox_telegram.run(...) in the server task set.
// inbox_telegram.run(cancel.child_token(), Duration::from_secs(2)).await
```

`storage`, `inbox_store` and `identities` must share the same owning database;
binding recovery rejects a mismatched Inbox/text database path. Use the same
`groups_enabled` policy on `TelegramAdapter::with_group_access` and InboxRuntime.
Do not install a mock or disabled Telegram transport merely to activate menus.
Install the same IdentityStore on TelegramAdapter for trusted non-owner incoming
routes as well as on InboxRuntime.

Public construction and lifecycle APIs:

- `TelegramAdapter::text_delivery(Arc<yoink_storage::Store>, Duration)` returns
  `Result<TextDelivery, StoreError>`, sharing the prepared bot/client and verified
  bot ID. The prepared HTTP client disables redirects.
- `TextDelivery::new(Bot, i64, Arc<Store>, Duration)` is available for trusted
  alternative construction/testing. Supply a client with redirects disabled and
  no retry middleware. `timeout()` exposes the total request deadline.
- `TelegramAdapter::with_inbox(InboxRuntime)` installs command handling only.
- `InboxRuntime::new(InboxStore, Arc<IdentityStore>, TextDelivery, bool)` is
  infallible construction; `with_config(InboxRuntimeConfig)` validates bounds.
- `InboxRuntime::run(self, CancellationToken, Duration)` is an owned future
  returning `yoink_inbox::Result<()>`; interval must be in `(0, 60s]`.
- `tick(&self)` and `reconcile_bindings(&self)` are explicit bounded recovery APIs.

## Configuration and shutdown

Text request timeout is `(0, 60s]`. Connection and streamed JSON receipt reading
share this deadline. Maximum response size is **64 KiB** (with or without a
Content-Length header). Outgoing plain text is validated at 4096 UTF-16 code
units; Inbox rendering uses a conservative 4000-unit bound.

Defaults: notification lease 120s, recovery batch 64, dispatch batch 1. Lease
must strictly exceed text timeout and be at most 3600s; batch bounds are 1–256
for recovery and 1–16 for dispatch. A tick bounds receipt recovery and completion
recovery separately by recovery_batch, and new claims by dispatch_batch.

Cancellation stops admission between dispatches. An admitted request finishes
its bounded attempt and records the outcome before the loop returns. Cancel the
runtime token and drain this future before closing the database. Budget drain
for the request deadline plus bounded DB acquisition/persistence and in-flight
Telegram update handlers; a conservative tick allowance is dispatch_batch times
the request timeout plus DB work. An outer forced abort remains safe but leaves
an admitted ledger row unknown. Do not convert that row to retryable work.

Expected pre-send grant/ban/group/topic/membership/provenance denials cancel only
a fenced live claim. Stale admission is a no-op. Routine revocation does not
terminate the runtime. DB and invariant failures remain fatal/actionable.

## Durable effects and authority

Migration23 adds only `runtime_text_effects` and its recovery index; native
schema registration expects 67 tables including migration bookkeeping, or 66
when excluding `runtime_migrations`. Existing migration checksums are untouched.

A text identity includes verified bot ID, original chat/topic/message, actor,
optional item, send versus edit, rendered text, and the immutable application
payload. Replayed commands recover their original stored response without re-running
mutations or re-rendering a changed locale/feed. Initial command keys identify
bot/chat/topic/incoming message; changes
to actor/item/address/payload conflict. Completion keys use the exact Inbox
`send_token` unchanged, with no prefix or hashing.

`/save` holds one BEGIN IMMEDIATE transaction for command lookup, item ingestion,
enrichment job admission, rules/bridges, and the immutable localized response.
`Store::ingest_on` uses the caller-owned connection and never commits or acquires
another connection. `Store::prepare_text_on` writes the original item ID,
normalized URL, `created` result and `bind_item` eligibility to a `prepared` row
in that same transaction. A competing/replayed update reuses this original
response instead of re-ingesting. Distinct incoming message IDs still produce
duplicate acknowledgments with `created=false` and `bind_item=false`.

`prepared` means no external attempt was admitted, not Unknown. `begin_text`
atomically moves the exact immutable prepared identity to sending (or inserts
sending for nonprepared effects) before one external request. A trusted receipt must
match bot sender, chat kind/ID, topic, positive message ID, echoed plain text,
and exact edited message ID. A send cannot claim the incoming message ID as its
receipt. Confirmation is persisted before binding an item reply. Missing,
malformed, oversized, error or lost responses remain unknown and never retry.

Confirmed initial receipts are bound in a transaction, without requiring fresh
grants to record the existing effect. First binding wins; waiting classification
notifications are awakened even when classification happened first. Duplicate
save acknowledgments, usage/read replies and denial feedback cannot become
completion targets. No fallback binds an incoming user message.

Completion delivery obtains bot/topic provenance from a confirmed initial
receipt and rechecks shared effective feature grants, bans, group enablement,
thread policy, bot membership generation and actor membership at dispatch.
Menus use fresh feature eligibility but do not authorize execution.

`seal_text_absence` inserts an atomic permanent tombstone before asserting
not-sent. Late dispatch against a tombstone makes no request. Sending/unknown
rows are never absence proof. Unknown edits continue to fence later edits through
Inbox sequence ordering. Confirmed completion effects reconcile without fresh
grants and without another edit. A changed persisted notification snapshot or
address against a confirmed effect is an invariant failure, not a resend.

## Checks and integration gates

Focused recipes: `format-inbox-transport`, `check-inbox-transport`,
`test-inbox-transport`; run with `CARGO_BUILD_JOBS=1`,
`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0` and one worktree target.
The recipes preflight resources. No host Python or workspace/image build is
needed. Retain this worktree target until the coordinator confirms repair integration.

Coordinator-owned gates remain server configuration/router/readiness wiring,
full-workspace checks, import/image/process verification and deployment behavior.
Unknown send/edit effects intentionally have no automatic retry or operator
"assume delivered" path. An operator reconciliation workflow must supply
independent trustworthy evidence rather than synthesizing a receipt.

Verified focused checks after the review repair: 266 transport-slice tests
(106 storage, 151 Telegram, 9 i18n), plus 63 Inbox-module tests:
**329 passed, 0 failed, 0 ignored**. There are 30 new contracts relative to the
isolated HEAD: 24 Inbox transport/runtime tests, five text-ledger tests and one
feature-aware menu test. Focused clippy with warnings
denied passed for storage/Telegram/i18n and for Inbox; schema-source check and
diff whitespace check also passed. A dependency-only proc-macro-error2 future
incompatibility warning remains upstream. Server/workspace/image checks were
not run by this slice.

## Incremental review repair

Compared with the last submission, changes are confined to:

- `yoink-inbox/src/store.rs`: extract caller-transaction ingestion hook;
  normal ingestion still owns and commits its wrapper transaction.
- `yoink-storage/migrations-turso/0023_text_effects.sql` and `src/text.rs`:
  prepared state/model, transaction-scoped lookup/preparation, admission CAS,
  and prepared sealing. No new table/migration or table-count change.
- `yoink-telegram/src/inbox_runtime.rs`: atomic save-result preparation/replay,
  and one captured timestamp for outcome and not-sent retry deadline.
- `yoink-telegram/src/inbox_runtime_tests.rs` and this handoff document.

Increment: **five tests**, all passed. Post-admission revocation is controlled by
an admission gate and advancing clock; its token seals with zero dispatch while
the owned loop continues and delivers the next eligible notification. The crash
test aborts the handler after application+prepared commit but before send CAS,
then fully closes/reopens the database; original English creator response and
binding eligibility survive a changed locale. Real enrichment/classification
admission produces a waiting notification which confirmed receipt recovery
wakes. Concurrent handlers rendezvous before transaction entry, then the creator
pauses after commit so the other handler wins external admission using the same
creator result. Exactly one item/enrichment job and one Bot API request result;
later real classification remains deliverable. A distinct duplicate never binds.
Preparation failure rolls back item, job and ledger writes together. The storage
regression rejects confirmation before admission, changed identity, duplicate
CAS admission and late admission after prepared sealing.

Target retained after final checks; no server, image, full-workspace or deployment
checks were performed for this repair. Migration23 remains uncommitted and is
not compatible with the earlier uncommitted checksum on an already-migrated
scratch database; coordinator must recreate that scratch DB rather than mutate
migration bookkeeping.
