# Native downloader NSFW policy

## Delivery boundary

Rust owns authorization and classification. Every primary fresh-upload or cached-file-ID delivery unit reloads the native `nsfw_domains` / `nsfw_keywords` tables and current destination policy from a consistent database snapshot, after the existing identity and credential rechecks and before `begin_delivery_unit`. No policy verdict is persisted in request snapshots or accepted from Python.

Classification preserves the legacy checker's three layers:

1. Domain equality or subdomain suffix (not substring matching). Domain comparison is case-insensitive, strips a leading `www.`, supports IDNA and optional authority ports, and matches host rules independently of a URL's port.
2. URL keyword matching uses ASCII alphanumeric delimiters. Multiword rules accept non-alphanumeric URL separators; `_` is a delimiter. Python-compatible case-insensitive special-letter handling is retained. Keywords are literal, never regex programs.
3. Source title, the first 2000 description characters, tags and categories use Unicode prose word boundaries, with `_` treated as a word character. Source metadata comes from extractor dictionaries, not FFprobe or filenames.

Classified content is blocked when the current group `nsfw_allowed` is false. Groups never receive spoilers. Private media uses the current `dl_user_settings.nsfw_blur` (legacy default true if no settings row exists). Telegram spoilers are set only for photo and MP4-video sends and album entries, never audio/documents or file-mode sends. Private blur is editable in the existing versioned native settings menu or with `/settings nsfw_blur true|false`; this preserves unrelated preferences and credentials.

Unreadable or malformed policy, missing group policy, and fresh media lacking bounded metadata fail closed. Policy checks do not grant chat/feature/credential access or relax network safety. A source format report is a fixed-schema document without source prose/media and remains outside media classification.

## Cache and durability

Confirmed media cache entries retain bounded source prose alongside the validated opaque Telegram file IDs, never a sticky classification boolean. Rule additions/removals and group/blur changes therefore take effect even on a cache hit and after restart. Old cache entries without source metadata are misses and must be refetched; they cannot be delivered as unclassified cached media. Malformed cache entries retain the existing download fallback, but no malformed cached IDs or metadata are sent.

The protocol still uses the v2 envelope; metadata is an optional descriptor field for wire compatibility, but is mandatory at the media delivery boundary. Existing file, MIME, workspace, frame-size, quota, request-snapshot and credential checks remain in place. Debug output redacts source prose.

Migration `0016_content_policy.sql` adds only the runtime operation `content_failure` column. Its values are `nsfw_blocked` and `nsfw_policy_unavailable`. EN/RU `/job` and progress text show the classified reason. Policy stops are not retryable. A previously confirmed prefix remains `delivery_partial`; existing unknown-send outcomes and receipt fencing are unchanged. No media unit is marked sending merely to perform its policy check.

Multi-item metadata is conservatively combined across the batch. Worker aggregate limits fail rather than dropping a later item's evidence; this can refuse unusually verbose playlists/galleries instead of sending partially classified content. No image inference or provider-specific age-rating classifier is introduced.

## Existing frontend rule API

Authenticated moderator/admin/owner callers use the existing paths beneath `/api/v1/dl/nsfw`:

- `GET/POST domains`, `PATCH/DELETE domains/{id}`
- `GET/POST keywords`, `PATCH/DELETE keywords/{id}`
- `POST check` with `{url}`; response includes `url`, `is_nsfw`, `matched_domain`, `matched_keyword`, and `matched_keywords`
- `POST import` with optional domain/keyword arrays; response has `domains_added` and `keywords_added`
- `GET export` with domain/keyword objects and notes

List and CRUD response shapes, IDs, notes, creation timestamps, 201 creation and 204 deletion statuses match the existing downloader frontend. Import skips duplicates and preserves existing notes. Reads/imports do not rewrite imported rows. All SQL remains in `.sql` files.

Authentication extracts only the validated JWT subject. Every management read/write rechecks the current database role and ban, with the configured-owner handling used by core administration. Writes hold an IMMEDIATE transaction throughout authorization, revision checking and mutation. A still-valid JWT claiming moderator privileges cannot overrule a database demotion or ban.

List/export responses expose an `ETag` containing the global rule revision; writes optionally accept `If-Match`. Supplied stale revisions return 409. Absent revisions preserve the existing unversioned frontend contract. The revision lives in a dedicated `bot_settings` key, advances transactionally, and malformed/overflow revision state refuses mutations. There is no in-process rule cache or reload requirement.

This work covers content-policy delivery compatibility and rule administration, not a full port of the legacy `/nsfw` command menu. The legacy one-shot `/nsfw mark` command is not added: it would require an explicitly snapshotted per-request override rather than untrusted worker classification or transient state affecting cache delivery. No forwarding, redirects, topics, providers, or service activation are added.

## Focused checks

Run from `backend`:

```sh
just test-nsfw
just test-nsfw-regression
just check-nsfw
just test-nsfw-worker
just test-nsfw-worker 'test_*.py'
```

Rust/database tests use disposable private temporary directories. Python runs exclusively in an isolated, network-disabled Docker container, using the existing worker image and read-only source mounts. These recipes do not build shared image tags, start services, or access production databases. End-to-end Docker acceptance and integration remain coordinator-owned.
