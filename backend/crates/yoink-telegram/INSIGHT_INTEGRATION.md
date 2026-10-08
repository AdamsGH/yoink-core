# Native Insight Telegram bridge

## Composition-root contract

Inject the **same** `yoink_insight::Insight` instance used by HTTP/activity, the
same `Arc<IdentityStore>` used by membership polling and menus, and the same
`Arc<yoink_storage::Store>` used by durable media/Inbox delivery. No environment
loader, credential resolver, provider, executor, or background Insight loop is
created here.

Public API:

```rust,ignore
use yoink_telegram::{InsightRuntime, TelegramAdapter};

// Existing prepare / media / identity / group configuration happens first.
let delivery = adapter.text_delivery(shared_ledger.clone(), text_timeout)?;
let insight_runtime = InsightRuntime::new(
    shared_insight.clone(),
    shared_identities.clone(),
    delivery,
    groups_enabled,
);
let mut adapter = adapter.with_insight(insight_runtime.clone());
// Apply the composition root's startup deadline to this existing hook.
adapter.initialize_menus().await;
// Set server readiness only after startup reconciliation.
adapter.run(shutdown.clone()).await?;
```

Exact signatures:

- `InsightRuntime::new(service: Insight, identities: Arc<IdentityStore>,
  delivery: TextDelivery, groups: bool) -> InsightRuntime`
- `TelegramAdapter::text_delivery(&self, ledger: Arc<Store>, timeout: Duration)
  -> Result<TextDelivery, StoreError>`
- `TelegramAdapter::with_insight(self, runtime: InsightRuntime) -> Self`
- `TelegramAdapter::initialize_menus(&mut self)` (async)
- `TelegramAdapter::run(self, shutdown: CancellationToken)
  -> Result<(), TelegramError>` (async)
- `InsightRuntime::cancel_all(&self)` (synchronous, permanent cancellation)

`new` does not spawn tasks. There is **no separate Insight runtime `run()`**.
The adapter owns admitted command futures. Shutdown/polling failure cancels
Insight before dispatcher draining; returning/dropping `run` cancels its runtime
lifetime guard. Normal teardown must cancel the provided token and **await**
`adapter.run`, not abort its task, so in-flight transport receipt classification
can drain. `cancel_all()` may be used explicitly but does not await draining;
await the adapter future afterward. A cancelled runtime cannot be restarted.
Keep the existing identity/menu and opt-in group builders; pass the same group
flag to `InsightRuntime::new`. Runtime and delivery databases must match.

Without injection there are no Insight commands or menu entries. The shared
identity notifier remains the sole menu refresh source. Menu reconciliation and
builtin help read current shared product settings (including grant/BYOK
readiness), not a Telegram-owned grant cache. The Telegram status display no
longer lists injected Insight among planned modules; the coordinator still owns
server/system module descriptors and global readiness.

## Dispatch, authority and delivery

`/about`, `/summary`, `/tldr` and their own-bot suffixes use the existing shared
product parser and `execute_stream`. Only observed sender/chat/topic and the
unaltered message text reach Insight. Bot senders, sender-chat messages and
non-admitted group routes are rejected by existing routing. Each outbound draft,
chunk/edit and error refreshes current identity, membership, group/thread policy
and shared product permission. Final BYOK output also requires the current
shared BYOK view to remain enabled and tested even if an explicit feature grant
would permit a different route.

The shared `TextDelivery` ledger fences external attempts and validates bot/chat/
thread/message receipts. Any existing initial admission fences replay, including
unknown or abandoned effects. Unknown sends/edits stop the command; they are not
retried. Edits target only confirmed bot message receipts. Delivery is plain text,
with localized incomplete/complete headers and sanitized localized errors. Each
chunk stays below 4000 UTF-16 units including its header; chunking iterates Unicode
scalar values and does not split surrogate pairs. Incremental drafts are flushed
at legacy-like intervals; an eight-second pending draft exposes the cancellation
ID for slow work. The thinking timer never waits on a lock held by the suspended
execution future. Buffered content is bounded at 1 MiB and admission at 32 active
commands.

`/cancel <uuid>` recognizes only active Insight work belonging to the current
actor/chat/topic; unmatched identifiers retain existing media cancellation
handling. Insight commands and cancellation use unkeyed dispatcher distribution
when injected so cancellation can run while the same chat's provider call is
pending; unrelated command dispatch retains chat-keyed distribution. Revocation
and cancellation do not publish a final completion/error. Private restricted
subjects with actual Insight authority can obtain product-only `/help`/`/start`
through durable delivery, without receiving media authority. Insight-only groups
can likewise obtain current product help under current group policy.

## Locked integration checks

The coordinator integrated this bridge into the shared server runtime before
menu initialization. The HTTP and Telegram dependency edges are reconciled in
Cargo.lock. Downloads now handles FullContent executor errors without retry.

From `backend/`, the original main checkout passes:

```sh
just check-crate yoink-telegram
just lint-crate yoink-telegram
just test-crate yoink-telegram
```

All 166 Telegram tests pass. Clippy passes with warnings denied. These results
include existing media, commands and Inbox regressions, current group/topic
authority, real loopback SSE fragmentation, durable receipt fencing, revocation,
actor/chat/topic cancellation, current BYOK menus and restricted help. The
registry proc-macro-error2 future-compatibility notice remains nonblocking.

Library tests do not prove actual confined-worker, Gemini or built-SPA behavior.
The process fixture must also force a server shutdown deadline while a Bot API
receipt is stalled and check handler cleanup and replay fencing. The ordinary
shutdown test does not establish forced-abort behavior.
