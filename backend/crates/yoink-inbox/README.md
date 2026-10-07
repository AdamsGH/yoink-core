# Native Inbox module

Module-local acceptance is complete for the owned HTTP/use-case/storage/recovery
contracts and deterministic source regressions described in [PARITY.md](PARITY.md).
This is an application crate, not a standalone server or a deployed-product
cutover claim. [INTEGRATION.md](INTEGRATION.md) is the exact coordinator wiring,
frontend recovery/key contract and notification-ledger handoff.

Preserved original Inbox/Insight plugin/frontend snapshots and the initial
28-test slice remain untouched. Real shared adapters, central runtime/frontend,
Telegram renderer/stream/send ledger and global migrations are coordinator-owned.

## Focused verification

From `backend`, Cargo jobs=1, one reused `target`:

```sh
just format-inbox
just check-inbox
just test-crate yoink-inbox
```

From `backend/crates/yoink-inbox`:

```sh
just source-oracle
just postgres-oracle
```

Latest focused result: **63 Rust tests pass**, Clippy warnings denied; preserved
Pydantic/GitHub/chrome oracle matches; 16 PostgreSQL C.UTF-8 ILIKE cases pass.
Typed checks/oracles include resource preflight. Oracles use existing images,
read-only source and disposable/no-network storage: no host Python, external
writes, image builds/pulls, production DB or credentials. Only explicit
`just source-oracle-record` regenerates owned golden fixture bytes.

## Durability boundaries

- Ingest, classification and star snapshots commit under fenced job leases.
- GitHub intent is durable before send. Sending/unknown never automatically
  resends; explicit candidate adoption is required for uncertain creates.
- `POST /folders` defaults to local-only. Explicit remote creates **require** a
  stable user-scoped `Idempotency-Key`; response loss must reuse the same key and
  payload, never start a new logical create. Ordinary folder PUT/PATCH/DELETE are
  source-compatible local actions; remote rename/delete have explicit endpoints.
- Classification result, resolved notification snapshot and job completion commit
  together. Confirmed save receipt binding can happen before or after completion.
  Outbox claim/retry is safe only before its durable send boundary. Ambiguous
  edits need shared delivery-ledger proof; absence of a receipt is not retry proof.
- An unknown create with no candidate remains fail-closed. Local Inbox operations
  still work. There is no invented “clear unknown and create again” endpoint.

Apply all three owned SQL table proposals through explicit shared migration
registration before enabling this module; no global revision is reserved here.
