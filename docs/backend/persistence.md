# Persistence and recovery

Rust persistence has two separate roles: [identity snapshots](../../backend/crates/yoink-identity/src/lib.rs) read selected legacy rows, while [yoink-storage](../../backend/crates/yoink-storage/src/store.rs) owns durable operation state in `yoink_runtime.operations`. SQLx does not own legacy `public` tables.

## Admission and idempotency

Admission inserts the request and its claimable queue state in the same PostgreSQL row. There is no separate database-to-queue publish step. The uniqueness key is `(actor_id, idempotency_key)`:

- The same actor and key with the same JSON request returns the existing operation ID and does not create another operation.
- The same actor and key with a different request returns a conflict. Other actors have independent key spaces.
- Keys must be non-empty ASCII, at most 128 bytes, and contain no control characters.

This protects admission from duplicate submissions. It does not make downstream effects exactly once.

## Claims, leases, and retries

Workers claim queued rows in creation order using row locks with `SKIP LOCKED`. A claim increments the attempt and records a worker ID, random lease token, and expiry. Leases must be 1 to 300 seconds. Heartbeat and state changes require the matching operation, attempt, worker, and lease token, and a still-live lease. A late or replaced claimant cannot update the queue row.

On a later claim attempt, expired leases are reconciled: requested cancellation becomes cancelled, exhausted attempts become failed, and otherwise work returns to queued. A retryable failure also requeues only while attempts remain and cancellation was not requested. This is at-least-once execution: a worker may perform an external action and die before recording completion. Fencing protects PostgreSQL state, not remote uploads or other external side effects. A future executor must make those effects safe to retry.

## Cancellation and outage recovery

Cancellation records a request for the owning actor. Queued work is cancelled in storage; running work receives a cancellation flag for its worker to observe and acknowledge. The flag cannot force-stop a remote operation, so cancellation does not promise that an upload or other remote effect stops immediately.

PostgreSQL preserves admitted operations across process restarts. Once the Rust worker execution layer exists, a new worker can recover queued work and expired leases through claims. No automatic rollback or drop removes queue data. For migration authority and operator recovery, see [Database ownership](../decisions/0001-database-ownership.md).
