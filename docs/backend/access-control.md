# Access control and identity

Policy rules are implemented as typed values in [yoink-access](../../backend/crates/yoink-access/src/lib.rs). [yoink-identity](../../backend/crates/yoink-identity/src/lib.rs) builds read-only authorization snapshots and provides transactional registration, profile, settings and account/access administration. Opt-in frontend authentication uses these shared operations; media retains its separate command gates. Authenticated group/topic/member-policy administration is available. Tag-map, API-key administration and core M2M endpoints are available. Native Telegram membership discovery and automatic tag grants are available. Remaining configuration workflows and product API parity remain pending.

## Global roles and blocks

Roles are ordered `banned < restricted < user < moderator < admin < owner`. Non-positive user IDs fail closed. A banned role or a timed ban with `ban_until > now` blocks access before any grant or group override; equality with the current time is expired. A configured owner is elevated only after the snapshot has checked global blocking.

Feature grants are considered in this order:

1. Owner.
2. Matching persisted explicit grant that has not expired. `expires_at == now` is expired.
3. Registered feature's minimum role.
4. Matching provider grant.

Blocked principals and unknown/unregistered features do not gain access through these sources. The media check uses the combined snapshot authorization API with the required `user` role in the actual private or group scope. A group override or auto-grant can elevate a restricted user's effective group role but cannot bypass a global ban or change the user's global role.

## Media command authorization

When the native identity store is configured, private ingress accepts an identifiable non-bot sender only in their matching private chat, without a topic or `sender_chat`. Missing users register under the shared open/approved-only policy. Existing accounts are read without rewriting profile metadata or revisions. Pending users receive approval guidance; blocked users receive a fixed denial. Authorized ordinary users can request caller-owned media with supported non-secret flags, playlists, clips, subtitles and formats; edit their own nine-field settings/format preferences and language; use durable private quality/cut dialogs; and inspect or cancel their own scoped operations. Credential and system controls remain configured-owner-only. Without the identity store, private ingress remains configured-owner-only. Group commands require `YOINK_RUST_TELEGRAM_GROUPS_ENABLED=true` as well as downloads being enabled. The adapter ignores channel posts, bot senders and anonymous messages carrying `sender_chat`. Known users must qualify for the original registered group and topic. Missing or inaccessible identity/group data denies access. Group ingress does not register identities or mutate roles. `/status` stays owner-private.

The download service performs fresh read-only identity checks at admission, before a claimed operation starts, before artifact delivery and before progress edits. Checks apply the actual destination scope, minimum `user` role, group enablement, topic policy, role/block precedence and database-time expiry. Global blocking wins. Group enablement and topic rules apply to owners too. An explicit group role override takes precedence over an auto-grant, which only raises the effective local role. A changed or unavailable identity cannot inherit a prior decision. A revoked role, group or topic fails the operation before the next stage. Disabling the group opt-in also blocks persisted group jobs.

`/job <UUID>` and `/cancel <UUID>` require both actor ownership and the original bot/chat/topic. A query in another topic cannot reveal or cancel a job, even for its owner. Media uploads, albums, cached sends and admission replies retain that original destination. There is no private-chat or General-topic fallback. Private-delivery redirects and remaining legacy settings workflows are unimplemented. Bot membership events discover groups disabled until administrative approval.

## Telegram command menus

Startup reconciles native Bot API menus before readiness. Menus contain only implemented commands and follow the same owner, role, ban and group boundaries as handlers. Feature grants trigger a refresh but do not expose unimplemented commands or grant access to owner-only handlers. Global private fallback menus contain only `start` and `help`; global group fallbacks never advertise media.

Private chat menus use persisted account language. Refresh removes stale `en` and `ru` overrides before replacing the base menu. Group member scopes reflect effective local roles, group enablement and global blocks; private-only commands are excluded. Telegram has no topic-specific command menu, so topic authorization remains an execution check. Deleting a group revokes its tracked member menus.

Committed identity, language, role, ban, permission, tag-map and group-policy changes notify the adapter without an inbound Telegram message. The worker reloads committed state, serializes Bot API writes, honors `retry_after` and retries transient or applied-but-lost responses. Periodic reconciliation handles time-based expiry. Menu failure does not roll back account changes. Logs use fixed failure categories rather than credential-bearing API errors.

Initial reconciliation includes persisted restricted and banned users and runs within the existing startup deadline. Large imported user/group sets require an appropriately configured deadline because reconciliation considers registered users in each known group. Menus are discoverability, not authorization; every handler and delivery stage retains its own fresh access checks.

`just test-media menus` verifies private catalogs, HTTP-driven refresh, language overrides, transient failures, concurrent revocation and restart recovery. `just test-media group-admin` additionally verifies group member menus after grants, revocation, bans, disablement, restart and deletion.

## Frontend identity operations

Signed WebApp registration applies persisted open/approved-only onboarding atomically. New configured-owner accounts receive the owner role; existing accounts preserve roles, timed bans, preferences and grants. Blocking checks precede registration updates and owner elevation. Authenticated profile/settings requests reload current identity, so stale JWT roles cannot restore revoked access. Settings and signed profile updates advance the shared microsecond revision, invalidating older Telegram language callbacks. See [configuration](configuration.md#optional-frontend-authentication) for activation, verification bounds and the dangerous development-only impersonation switch.

## Account and access administration

With frontend authentication enabled, the native API implements user listing/detail/update, feature catalog and access views, explicit permission grant/revoke and bot settings at the existing `/api/v1` routes. User listing supports bounded pagination, search, role/status filters, sorting and download-count periods. Actor roles and bans are checked against current database state inside each operation, not trusted from JWT claims. Mutations run in Immediate transactions. Missing identity, unknown stored roles, corrupt policy and database failures do not authorize access.

Admins can manage ordinary users; owner-level targets and promotions require an owner. The configured owner cannot be changed through these APIs. User patches accept role, EN/RU language and a nullable timed ban only. Profile changes advance the shared microsecond revision, invalidating old Telegram language callbacks. Grant writes require a registered feature and preserve provenance protections: admins cannot replace or revoke tag-derived grants, while owners can. Active permission listings exclude expired grants. Global bans override role and explicit/provider feature access.

Bot settings reads require admin access and writes require owner access. The writable keys are `bot_access_mode`, `browser_cookies_min_role`, `inline_storage_chat_id` and `inline_storage_thread_id`, with closed policy values and validated IDs. Registration immediately respects persisted open/approved-only policy. Storing browser-cookie or inline destinations does not implement their remaining operational workflows. Tag-map editing and its feature dropdown catalog are implemented. Native membership events reconcile tag-derived access and Telegram command menus. Full product APIs remain pending. Feature grants do not bypass the separate minimum-role or configured-owner media command gates.

Tag-map reads and the available-feature catalog require admin access; full replacements require owner access. Entries retain Unicode labels without a new label-length limit. Replacements reject blank/control labels, duplicate tags and duplicate or unregistered feature keys before writing. Invalid requests preserve the prior map. Malformed persisted maps fail closed without implicit repair. Maps persist in the imported `bot_settings.tag_map` JSON format and survive restart. Effective tag grants use the current map and current memberships without overwriting explicit grant rows.

`just verify-db` covers native transactional administration and authenticated HTTP contracts. `just test-media admin` exercises owner administration through real HTTP and the fixture Bot API, including approved-only onboarding, immediate role/grant/ban enforcement, persisted policy across restart and localized Telegram replies. These checks use isolated data and fake credentials, not production activation.

## API-key administration

The existing `/api/v1/api-keys` list/create/revoke routes and `/api/v1/api-keys/scopes` catalog require a JWT-authenticated admin or owner. Every operation reloads current database authorization; a demoted or banned actor cannot use a stale token. Keys belong to a shared administrative listing, not a creator-only namespace. Revocation is idempotent, and missing keys return 404. Writes authorize inside Immediate transactions.

Creation uses 32 bytes of operating-system randomness, encoded as a `yoink_` URL-safe key. The raw secret appears only in the creation response. Storage retains its SHA256 hash and eight-character prefix; listing exposes neither secret nor hash. Omitted scopes default to `["*"]`; supported exact scopes and legacy namespace wildcards are retained, as are nullable expiry and microsecond timestamps. Invalid creation preserves existing records. Corrupt stored scope shapes fail closed without repair. List, creation and scope-catalog responses use `Cache-Control: no-store`.

`just test-media admin` verifies generation, hash/prefix storage, nonexposure, invalid requests, stale-role and ban enforcement, repeated revocation and restart persistence through the real HTTP listener and disposable fixture database. Native tests cover unique hashes, disk reopen and imported corrupt scopes. The separate `/api/internal/v1` M2M API authenticates these keys independently of JWT configuration.

## M2M API

Native status, user listing/detail, groups and event listing/creation retain the existing `/api/internal/v1` contracts. Requests require `X-Api-Key`, never a JWT. Authentication hashes the supplied secret, reloads revocation/expiry/scopes inside a fresh transaction and records successful authentication before scope evaluation. Missing, unknown, expired and revoked keys return 401 without updating usage; valid scope-denied keys return 403 and update usage. Exact scopes, namespace wildcards and `*` are supported. The creator's current role or ban does not invalidate a separate machine credential. Corrupt stored scopes fail closed. Responses are not cached and never expose secrets or hashes.

Status uses actual database counts, monotonic service age and the configured Telegram task's lifecycle and `getMe` username. JWT routes remain absent when frontend authentication is disabled. User/event pagination is bounded, groups retain legacy boolean query spellings and historical JSON-null event payloads return null without rewriting storage. Event writes preserve nullable user/group/thread IDs, object/null payloads and microsecond timestamps across restart.

`just verify-db` covers authentication, scope, expiry, revocation, imported JSON null, query validation and disk reopen. Rebuilt `just test-media admin` verifies the real HTTP endpoints, actual bot status, current counts, credential isolation, creator-role independence, usage timestamps, filtering and event persistence through restart.

## Group administration

Authenticated admins and owners can list, create, patch and delete groups and manage their topic and member policies through `/api/v1/groups`. New groups are disabled until explicitly enabled. Listing excludes private-chat records. Topics include nullable main-chat policies; a positive-topic whitelist denies unlisted topics. Member overrides stay local and never change the user's global role. Every operation reloads the actor's current role and bans; writes authorize inside an Immediate transaction. A stale token cannot restore demoted or banned administrative access. Group deletion cascades topic and member policies, without deleting historical downloads.

The existing frontend request and response shapes are supported, including positive private-chat storage destinations, nullable storage fields and 256-character Unicode names. Stored `allow_pm`, NSFW and storage settings do not implement private-delivery redirects or other unported workflows. Group photos are not implemented. Bot membership events discover disabled groups; they do not enable groups or overwrite administrative policies.

`just test-media group-admin` checks real HTTP mutations against Telegram admission and delivery, topic whitelist behavior, local role isolation, global bans, revocation, cascading deletion and persistence across restart. It runs against the rebuilt fixture image with disposable data. Native HTTP regressions cover stale-token demotions and timed bans across every group-policy operation.

## Telegram membership and tag grants

The polling listener retains raw Bot API member tags that the typed Telegram client does not expose. It commits membership observations before advancing the polling offset. A failed batch does not acknowledge its updates or dispatch a partial batch. Replayed observations are harmless; ordering uses event time and update ID per source, group and subject. Private bot block/unblock events are not group observations.

Bot membership discovers disabled groups. Administrators must enable them explicitly. Member observations register missing non-bot identities under the current onboarding policy without changing existing global roles, profile language or administrative local overrides. Known member absence and bot absence deny group execution and menus, including for owners. Imported groups with unknown presence retain their existing policy behavior. Ordinary messages do not establish membership.

Tags contribute global feature grants only from eligible current memberships in enabled groups. The current validated tag map determines the features. Unknown tag data preserves a stored tag; an explicitly empty tag removes it. Leaving a group removes that group's contribution without removing overlapping contributions or manual/imported grants. A bot departure starts a new membership generation, so rejoining cannot revive old member tags. Fresh member observations are required. Malformed persisted policy fails closed.

Admins cannot revoke tag-derived grants. Owner revocation persists per-user, per-feature suppression across membership events and restart. An explicit manual grant clears that suppression. Permission listings expose deduplicated effective tag grants without fabricating persistent grant rows. Deleting a group suppresses event rediscovery until explicit administrative creation, and retains menu cleanup targets.

Migration v14 adds native membership, lifecycle and suppression state while preserving imported policy and grant rows. Older binaries reject the upgraded schema. Rollback requires a compatible binary or a pre-migration backup. Verification does not migrate production data. Telegram only sends ordinary `chat_member` updates to eligible bot administrators when explicitly requested; the poller requests them, but cannot reconstruct events it never received.

`just verify-db` covers ordering, transaction rollback, provenance, overlapping tags, suppression and the v13-to-v14 schema upgrade. `just test-media membership` verifies raw polling, disabled discovery, actual group delivery, tag revocation, stale events, bot lifecycle generations, deleted-group fencing, menu cleanup and persistence across restart with disposable fixture data.

## Read-only snapshot boundary

Identity lookup returns no snapshot for a missing user and fails on invalid IDs, unknown stored roles, or database read errors. Reads run in a read-only transaction and use database time for expiry decisions. The optional Insight provider requires a truthy legacy BYOK setting (`1`, `true`, `yes`, or `on`, case-insensitive), a non-null `tested_at`, and a null `test_error`. It exposes only a readiness boolean, never credential contents.
