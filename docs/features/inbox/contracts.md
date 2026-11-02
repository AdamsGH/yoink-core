# Inbox durability and compatibility

Inbox preserves caller ownership, saved metadata, explicit remote intent, and uncertain-effect recovery across restarts.

## Ingestion and classification

URL normalization and deduplication use caller-scoped identities. Ingestion, rule effects, and job admission commit together.

Content and classification publish under the active job lease. Manual categories and existing user metadata survive automatic updates.

Archive or deletion fences late jobs and cancels eligible pre-send notifications. A late confirmed receipt still belongs in the delivery journal.

## Folders and Stars

Local folder PUT, PATCH, and DELETE remain local, including for linked folders. Remote GitHub rename/delete require explicit operations.

Star synchronization validates complete bounded snapshots before applying removals. It preserves local pins, order, manual membership, and AI fields according to their ownership rules.

GitHub identity uses repository identity and numeric IDs rather than a display name alone.

## Remote mutation ledger

The service records intent before sending a mutation. Protected transitions separate prepared, sending, confirmed, and applied states.

Remote creates require a stable caller-scoped `Idempotency-Key`. Replaying the same key and payload returns the existing outcome.

Sending, unknown, response loss, or a crash does not prove non-acceptance. These states cannot automatically authorize another external attempt.

Candidate adoption must refer to an already observed compatible remote object. No candidate means fail-closed recovery, not permission to clear unknown state.

## Notifications

Classification results, resolved notification payloads, and job completion commit together.

A confirmed save receipt can bind before or after classification. Recovery serializes effects for the same reply and uses durable text-send/edit evidence.

Retry is safe only before the durable external-send boundary. A missing receipt alone does not prove that Telegram rejected a request.

## API compatibility

The native API retains separate full replacement and partial update semantics. Unknown inert rule triggers do not become executable code.

Item and folder reads enforce current caller/team visibility. Public DTOs do not expose stored credentials or trusted Telegram targets.

See [the native crate](../../../backend/crates/yoink-inbox/), [runtime ownership](runtime.md), and [Telegram transport](telegram.md).
