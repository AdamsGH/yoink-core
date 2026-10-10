# Forced final receipt gate: integration contract

The owned module exports `forcedReceipt(c)`. It fails closed in preflight until all five context adapters below exist. Node regressions exercise assertions and modeled orchestration only, not native process acceptance.

## Existing source evidence

- `yoink-telegram/src/text.rs`: confirmation hold enters after the real Bot API response passes sender/chat/text proof, outside the five-second request timeout, immediately before durable `confirm_text`.
- `insight_runtime.rs`: this cached reply sends directly, with final receipt token `insight:123456:44:0:39501:0:0`. Uncached private previews seal revision 0 and send the final at revision 1. Any initial effect fences replay before product execution. Handler ownership survives Telegram adapter abort and is joined by `drain_handlers`.
- `lib.rs::insight_distribution`: Insight concurrent commands return `None`, while `/start` uses chat distribution. Thus a later `/start` response is a live-handler witness but **not** an ordering barrier for a concurrent `/tldr` replay. getUpdates acknowledgement, queue-empty, unchanged ledger, and stopped-handler drain do not prove that this exact replay handler reached its fence.
- `runtime.rs`: exact adapter task ID is logged on spawn and cancellation join. Handler drain, concrete Insight shutdown and database shutdown are separately logged.
- `Draft::flush(true)` sends the source-linked bold `Summary` header and rendered body without a cancellation command. The gate validates that final header and unchanged plain cached body. Cached execution emits no fake deltas. Private previews have no receipt; only the final send enters the confirmation hold.

## Required coordinator context API

Existing `launch`, `ready`, `stop`, `db.query`, `bot.enqueue`, `bot.calls`, `trusted.calls`, `trusted.origin`, `request`, `login`, `waitFor` retain existing semantics. Run with the original app ready; configure uses real owner login and HTTP, and cache priming uses granted user 44. Restart must preserve token/bot identity, user, database volume, settings, cache and provider/call history. The fixed message 39501 requires fresh fixture state. Do not set focus: native requests with focus deliberately bypass caching.

New required adapters:

- `await c.confirmation(mode, token='')`: use native authenticated fixture control (not SQL or JS hold); modes `confirmation_arm`, `confirmation_status`, `confirmation_release`. Return the native `{ok:true,hold:{token,state}}` response. The mode's token goes in controller `sql`, authentication remains the fixture secret. Status uses empty token. Hold duration stays 120 seconds.
- `await c.processLogs()`: complete logs for current original process, **retaining the just-stopped original's logs after `finishForcedStop` until the next launch**. Do not merge restarted logs.
- `await c.signalShutdown()`: send original SIGTERM only; return after delivery, do not wait for exit or adjust budgets.
- `await c.finishForcedStop()`: called only after exact adapter abort/join and successful hold release. Await original exit 1 with ShutdownTimeout, verify not running/no OOM/no SIGKILL/no Docker state error, detach original and its relay so stopped DB queries and restart use existing lifecycle. Preserve original logs. Do not issue another stop signal or force-remove the original to manufacture exit. Use a finite post-release handler-drain/exit deadline, not a fresh unbounded wait or another 90-second stop sequence. Verify workspace/child cleanup through the existing lifecycle.
- `await c.replayStatus(token)`: authenticated fixture-only exact-token observation returning `{token,fenced:N}`. Native controller currently exposes mode `replay_status` with `{token:fixtureSecret,sql:effectToken,mode:'replay_status'}` and response `{ok:true,token:effectToken,fenced:N}`. Assert native `ok === true` before returning the observation; do not manufacture zero or cache a value in JavaScript.

All controller/log/signal calls and DB helpers must have finite per-call deadlines. `waitFor` bounds polling, not an indefinitely pending callback. Keep these short enough for the entry-to-signal and arm-to-signal assertions. The scenario uses explicit 10-second entry, 100-second abort observation and 30-second live-witness/exact-replay polling bounds.

## Exact replay observation contract (parent-owned)

`await c.replayStatus(token)` returns `{token, fenced: N}`, an authenticated process-local counter for the exact initial text effect token. New process starts at zero. Increment only on the `Ok(Some(_))` existing-ledger branch in `InsightRuntime::handle`, after lookup and before its replay-fence return. Do not count lookup errors, update delivery, queue removal, handler admission, or `/start`. Bound storage to one observed token of 1..512 bytes in the fixture build, not an unbounded production registry. First read arms the observation; repeated reads must never reset it, and requesting another token must fail rather than replace it. The parent is implementing this outside the three owned files; its actual-process behavior remains unverified here.

The gate requires zero before enqueue, exactly one after enqueue, and a separate correlated `/start` Bot API response, then drains restarted handlers before asserting no duplicate and immutable receipt. Without this observation the module deliberately refuses to run, rather than hiding the gap with a sleep or a queue-empty predicate.

## Timing and result expectations

Arm occurs after genuine same-user HTTP cache priming: first response is uncached, second is cached with the same result. Signal delivery must finish within five seconds of observed entry and fifteen seconds of arm. The exact Telegram aborted-and-joined marker must appear no earlier than 90 seconds after starting signal delivery and within 100 seconds, while native hold remains `entered`. Release then allows the still-owned handler to confirm its already validated receipt. Required exact, unique log lines in order: adapter join, handlers drained, concrete shutdown, database close with `Err(ShutdownTimeout)`. Stopped ledger must therefore be confirmed with a non-null receipt. Restart must fence this same message and never send another final. Primary failures and hold-release cleanup failures are both reported; cleanup cannot turn failure into acceptance.

No changes to request20/text5/shutdown90/hold120, runtime, runner, server, Telegram, Cargo or images are made here. Remaining blockers are parent integration of the context adapters and scenario selector, independent review of native observation/lifecycle wiring, and matching-image actual-process acceptance. No Cargo, Docker, host Python, staging or commits were performed in this task.
