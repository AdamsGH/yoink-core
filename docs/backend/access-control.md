# Access control and identity

Policy rules are implemented as typed values in [yoink-access](../../backend/crates/yoink-access/src/lib.rs). [yoink-identity](../../backend/crates/yoink-identity/src/lib.rs) builds read-only authorization snapshots and provides transactional registration, profile and settings operations. Opt-in frontend authentication uses these shared operations; media retains its separate command gates. Full account administration and product API parity remain pending.

## Global roles and blocks

Roles are ordered `banned < restricted < user < moderator < admin < owner`. Non-positive user IDs fail closed. A banned role or a timed ban with `ban_until > now` blocks access before any grant or group override; equality with the current time is expired. A configured owner is elevated only after the snapshot has checked global blocking.

Feature grants are considered in this order:

1. Owner.
2. Matching persisted explicit grant that has not expired. `expires_at == now` is expired.
3. Registered feature's minimum role.
4. Matching provider grant.

Blocked principals and unknown/unregistered features do not gain access through these sources. The media check uses the combined snapshot authorization API with the required `user` role in the actual private or group scope. A group override or auto-grant can elevate a restricted user's effective group role but cannot bypass a global ban or change the user's global role.

## Media command authorization

When the native identity store is configured, private ingress accepts an identifiable non-bot sender only in their matching private chat, without a topic or `sender_chat`. Missing users register under the shared open/approved-only policy. Existing accounts are read without rewriting profile metadata or revisions. Pending users receive approval guidance; blocked users receive a fixed denial. Ordinary users can request basic video/audio/image downloads and inspect or cancel their own scoped operations. Extended flags, playlists, clips, subtitles, formats, settings, language and credential controls remain configured-owner-only. Without the identity store, private ingress remains configured-owner-only. Group commands require `YOINK_RUST_TELEGRAM_GROUPS_ENABLED=true` as well as downloads being enabled. The adapter ignores channel posts, bot senders and anonymous messages carrying `sender_chat`. Known users must qualify for the original registered group and topic. Missing or inaccessible identity/group data denies access. Group ingress does not register identities or mutate roles. `/status` stays owner-private.

The download service performs fresh read-only identity checks at admission, before a claimed operation starts, before artifact delivery and before progress edits. Checks apply the actual destination scope, minimum `user` role, group enablement, topic policy, role/block precedence and database-time expiry. Global blocking wins. Group enablement and topic rules apply to owners too. An explicit group role override takes precedence over an auto-grant, which only raises the effective local role. A changed or unavailable identity cannot inherit a prior decision. A revoked role, group or topic fails the operation before the next stage. Disabling the group opt-in also blocks persisted group jobs.

`/job <UUID>` and `/cancel <UUID>` require both actor ownership and the original bot/chat/topic. A query in another topic cannot reveal or cancel a job, even for its owner. Media uploads, albums, cached sends and admission replies retain that original destination. There is no private-chat or General-topic fallback. Private-delivery redirects, group registration and remaining legacy settings workflows are unimplemented.

## Frontend identity operations

Signed WebApp registration applies persisted open/approved-only onboarding atomically. New configured-owner accounts receive the owner role; existing accounts preserve roles, timed bans, preferences and grants. Blocking checks precede registration updates and owner elevation. Authenticated profile/settings requests reload current identity, so stale JWT roles cannot restore revoked access. Settings and signed profile updates advance the shared microsecond revision, invalidating older Telegram language callbacks. See [configuration](configuration.md#optional-frontend-authentication) for activation, verification bounds and the dangerous development-only impersonation switch.

## Read-only snapshot boundary

Identity lookup returns no snapshot for a missing user and fails on invalid IDs, unknown stored roles, or database read errors. Reads run in a read-only transaction and use database time for expiry decisions. The optional Insight provider requires a truthy legacy BYOK setting (`1`, `true`, `yes`, or `on`, case-insensitive), a non-null `tested_at`, and a null `test_error`. It exposes only a readiness boolean, never credential contents.
