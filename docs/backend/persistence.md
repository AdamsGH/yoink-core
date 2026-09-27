# Persistence and recovery

Rust persistence has two roles: [identity snapshots](../../backend/crates/yoink-identity/src/lib.rs) read selected legacy rows, while [yoink-storage](../../backend/crates/yoink-storage/src/store.rs) owns durable operation state in `yoink_runtime.operations`. SQLx does not own legacy `public` tables.

## Admission and idempotency

Admission inserts the request and its claimable queue state in PostgreSQL. The uniqueness key includes actor and idempotency key, and identical repeats return the existing operation while a differing request conflicts. The Telegram adapter builds its source-message idempotency key from bot ID, chat ID, and Telegram message ID. Thus a redelivered update for the same bot/chat/message maps to the same operation. This key does not make downstream effects exactly once and must not be used across live Rust and Python pollers sharing one bot: they may consume competing updates and the same update could be admitted independently by each service.

Keys must be non-empty ASCII, at most 128 bytes, and contain no control characters. The database applies global and per-actor pending quotas. Attempts and retry delay are bounded by configuration. Retryable download failures and expired worker leases can return work to the durable queue while attempts remain.

## Claims, leases, and cancellation

Workers claim queued rows in creation order using row locks with `SKIP LOCKED`. A claim increments the attempt and records a worker ID, random lease token, and expiry. Heartbeat and state changes require the matching operation, attempt, worker, and lease token, and a live lease. A stale claimant cannot change PostgreSQL state. This fences queue updates, not external effects.

Cancellation records a request for the owning actor. Queued work is cancelled in storage. During download, cancellation stops and reaps the worker process before acknowledging cancellation. Once Telegram upload has started, delivery outcome wins: the service waits for the delivery outcome or bounded delivery timeout and does not retry an uncertain upload because cancellation arrived mid-send.

## Delivery state and uncertainty

The queue enters `delivering` before Telegram `sendDocument` begins. A positively accepted response with a valid message ID is recorded as `succeeded` with that message ID. A known definitive Bot API rejection is recorded as `failed`. Unrecognized API errors (including server-side errors), a dropped/ambiguous response, timeout, process crash, or stale delivery lease are not safe to replay: the operation becomes terminal `delivery_unknown` and is not automatically resent. PostgreSQL lease fencing cannot make Telegram delivery exactly once.

When `/job <UUID>` reports `delivery_unknown`, check the chat before submitting a new request. A new request may create a duplicate delivery if Telegram accepted the first upload but its response was lost. This warning is deliberate, not a promise that queue retries prevent duplicate effects.

## Outage recovery

PostgreSQL preserves admitted operations across process restarts. Queued operations and expired leases are reconciled by later claims. Download execution may be retried within configured attempt and delay limits. Delivery uncertainty is terminal to avoid an automatic repeated send. There is no automatic database rollback or drop. For migration authority and operator recovery, see [database ownership](../decisions/0001-database-ownership.md).
