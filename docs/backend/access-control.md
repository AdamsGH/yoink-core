# Access control and identity

Policy rules are implemented as typed, I/O-free values in [yoink-access](../../backend/crates/yoink-access/src/lib.rs). [yoink-identity](../../backend/crates/yoink-identity/src/lib.rs) builds a read-only snapshot from selected legacy tables. This is not finished authentication: no authenticated business endpoints currently consume these decisions.

## Global roles and blocks

Roles are ordered `banned < restricted < user < moderator < admin < owner`. Non-positive user IDs fail closed. A banned role or a timed ban with `ban_until > now` blocks access before any grant or group override; equality with the current time is expired. A configured owner is elevated only after the snapshot has checked global blocking.

Feature grants are considered in this order:

1. Owner.
2. Matching persisted explicit grant that has not expired. `expires_at == now` is expired.
3. Registered feature's minimum role.
4. Matching provider grant.

Blocked principals and unknown/unregistered features do not gain access through these sources. The registered catalog is code-defined in `registered_features()`; a feature being registered does not imply it is wired to an authenticated endpoint.

Use `IdentitySnapshot::authorize(policy, feature)` for the combined decision. It checks scope, blocks, and role before resolving a registered feature, and uses the context-local role for the feature threshold. `effective_grant` is a lower-level primitive whose `FeatureSpec` must come from the trusted catalog, not request input. Identity snapshots are request-local, not a cache of permanent access rights.

## Contextual access

An `AccessPolicy` states required role, allowed private/group scopes, and whether to enforce group-enabled and thread restrictions. A group-specific role override takes priority over auto-grant. Auto-grant applies only to a restricted principal in an enabled group, and only when the configured role is higher. Contextual elevation affects that one decision; it does not mutate the global principal or persist the legacy side effect of changing a user's role.

Group thread policy permits a thread when its explicit matching rule is enabled, or, if no matching rule exists, when the group has no enabled thread restrictions. Group checks still obey global block precedence and policy scope restrictions.

## Read-only snapshot boundary

Identity lookup returns no snapshot for a missing user and fails on invalid IDs, unknown stored roles, or database read errors. Reads run in a read-only transaction and use database time for expiry decisions. The optional Insight provider requires a truthy legacy BYOK setting (`1`, `true`, `yes`, or `on`, case-insensitive), a non-null `tested_at`, and a null `test_error`. Merely saving a credential is insufficient. It exposes only a readiness boolean, never selects or returns credential contents. A ready BYOK setting is not proof that a feature endpoint or authenticated workflow exists.
