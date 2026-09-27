# Access control and identity

Policy rules are implemented as typed values in [yoink-access](../../backend/crates/yoink-access/src/lib.rs). [yoink-identity](../../backend/crates/yoink-identity/src/lib.rs) builds read-only snapshots from selected legacy tables. The first media slice consumes these decisions, but this remains a narrow experimental workflow, not general authenticated business endpoints.

## Global roles and blocks

Roles are ordered `banned < restricted < user < moderator < admin < owner`. Non-positive user IDs fail closed. A banned role or a timed ban with `ban_until > now` blocks access before any grant or group override; equality with the current time is expired. A configured owner is elevated only after the snapshot has checked global blocking.

Feature grants are considered in this order:

1. Owner.
2. Matching persisted explicit grant that has not expired. `expires_at == now` is expired.
3. Registered feature's minimum role.
4. Matching provider grant.

Blocked principals and unknown/unregistered features do not gain access through these sources. The media check uses the combined snapshot authorization API with the required `user` role and private scope, so blocked/restricted users do not qualify merely because the bot command parses.

## Media command authorization

The Telegram adapter has a separate outer gate that silently ignores messages unless they are from the configured owner in a private chat. Enabling downloads does not change that gate or open commands to all users.

The download service then performs fresh read-only identity checks at admission, immediately before a claimed operation starts, and again before artifact delivery. Each check applies the private-scope policy, minimum `user` role, role/block precedence, and database-time expiry semantics. A changed or unavailable identity cannot inherit a prior authorization decision. If the role becomes disallowed before execution or delivery, the operation is failed without starting that stage.

`/job <UUID>` and `/cancel <UUID>` are also actor-scoped in the service and storage layer. They cannot inspect or cancel another actor's operation. The current Telegram owner-only outer gate remains an additional restriction.

## Read-only snapshot boundary

Identity lookup returns no snapshot for a missing user and fails on invalid IDs, unknown stored roles, or database read errors. Reads run in a read-only transaction and use database time for expiry decisions. The optional Insight provider requires a truthy legacy BYOK setting (`1`, `true`, `yes`, or `on`, case-insensitive), a non-null `tested_at`, and a null `test_error`. It exposes only a readiness boolean, never credential contents.
