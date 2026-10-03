# Access control and identity

Policy rules are implemented as typed values in [yoink-access](../../backend/crates/yoink-access/src/lib.rs). [yoink-identity](../../backend/crates/yoink-identity/src/lib.rs) builds read-only authorization snapshots and provides transactional registration, profile, settings and account/access administration. Opt-in frontend authentication uses these shared operations; media retains its separate command gates. Authenticated group/topic/member-policy administration is available. Tag maps, API keys, remaining configuration workflows and product API parity remain pending.

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

## Account and access administration

With frontend authentication enabled, the native API implements user listing/detail/update, feature catalog and access views, explicit permission grant/revoke and bot settings at the existing `/api/v1` routes. User listing supports bounded pagination, search, role/status filters, sorting and download-count periods. Actor roles and bans are checked against current database state inside each operation, not trusted from JWT claims. Mutations run in Immediate transactions. Missing identity, unknown stored roles, corrupt policy and database failures do not authorize access.

Admins can manage ordinary users; owner-level targets and promotions require an owner. The configured owner cannot be changed through these APIs. User patches accept role, EN/RU language and a nullable timed ban only. Profile changes advance the shared microsecond revision, invalidating old Telegram language callbacks. Grant writes require a registered feature and preserve provenance protections: admins cannot replace or revoke tag-derived grants, while owners can. Active permission listings exclude expired grants. Global bans override role and explicit/provider feature access.

Bot settings reads require admin access and writes require owner access. The writable keys are `bot_access_mode`, `browser_cookies_min_role`, `inline_storage_chat_id` and `inline_storage_thread_id`, with closed policy values and validated IDs. Registration immediately respects persisted open/approved-only policy. Storing browser-cookie or inline destinations does not implement their remaining operational workflows. Tag-map editing, automatic Telegram group registration, API-key administration, Telegram command-list refresh and full product APIs remain pending. Feature grants do not bypass the separate minimum-role or configured-owner media command gates.

`just verify-db` covers native transactional administration and authenticated HTTP contracts. `just test-media admin` exercises owner administration through real HTTP and the fixture Bot API, including approved-only onboarding, immediate role/grant/ban enforcement, persisted policy across restart and localized Telegram replies. These checks use isolated data and fake credentials, not production activation.

## Group administration

Authenticated admins and owners can list, create, patch and delete groups and manage their topic and member policies through `/api/v1/groups`. New groups are disabled until explicitly enabled. Listing excludes private-chat records. Topics include nullable main-chat policies; a positive-topic whitelist denies unlisted topics. Member overrides stay local and never change the user's global role. Every operation reloads the actor's current role and bans; writes authorize inside an Immediate transaction. A stale token cannot restore demoted or banned administrative access. Group deletion cascades topic and member policies, without deleting historical downloads.

The existing frontend request and response shapes are supported, including positive private-chat storage destinations, nullable storage fields and 256-character Unicode names. Stored `allow_pm`, NSFW and storage settings do not implement private-delivery redirects or other unported workflows. Group photos and automatic Telegram registration are not implemented.

`just test-media group-admin` checks real HTTP mutations against Telegram admission and delivery, topic whitelist behavior, local role isolation, global bans, revocation, cascading deletion and persistence across restart. It runs against the rebuilt fixture image with disposable data. Native HTTP regressions cover stale-token demotions and timed bans across every group-policy operation.

## Read-only snapshot boundary

Identity lookup returns no snapshot for a missing user and fails on invalid IDs, unknown stored roles, or database read errors. Reads run in a read-only transaction and use database time for expiry decisions. The optional Insight provider requires a truthy legacy BYOK setting (`1`, `true`, `yes`, or `on`, case-insensitive), a non-null `tested_at`, and a null `test_error`. It exposes only a readiness boolean, never credential contents.
