# Remaining downloader controls: native contracts

## Executable source and scope

Traced the unchanged plugin files `commands/{clean,split,keyboard,mediainfo,subs,link,proxy}.py`, `api/routers/downloads.py`, `api/schemas.py`, `storage/{models.py,repos/settings.py}`, `url/pipeline/{download_phase,upload_phase}.py`, and frontend `types.ts`, `api/settings.ts`, `pages/settings/SettingsPage.tsx`.

Split, subtitle enabled/always-ask, and keyboard layout are stored preferences in the active legacy pipeline. They do **not** prove splitting, video subtitle embedding, source-selection prompts, or a post-download keyboard consumer. This implementation edits/preserves them without inventing those workflows. Existing native quality/cut, inline search, deferred modes, NSFW/mark, routing, credentials, quotas and replay remain separate.

## Service and transport contracts

- `UserSettingsService::new(Database, Arc<IdentityStore>)` is database-only. HTTP settings do not require a bot, worker, downloader process or outbound request.
- `GET/PATCH /api/v1/dl/settings` use the authenticated ID and current database authorization, never a supplied user ID or JWT role. Like legacy self-settings, authenticated non-blocked restricted identities can edit their own preferences; that does not authorize downloading or shared cookies. Telegram commands still require downloader access and the actor's private, current-bot, non-thread target.
- GET returns the existing frontend fields including caller-owned `proxy_url`, `audio_codec`, `updated_at` (UTC), and computed `has_pool_access`. It omits imported engine arguments and forwarding state. Responses carry `Cache-Control: no-store` and an ETag derived from the settings revision.
- PATCH is a closed typed `UserSettingsPatch`. Null fields are ignored, including `proxy_url: null` (legacy does not clear it). An optional `If-Match` performs atomic compare-and-swap. One parameterized UPSERT preserves unrelated/imported fields and rechecks blocking at mutation. No arbitrary engine arguments or role/capability changes are accepted.
- Proxy editing accepts the existing guarded HTTP(S) proxy URL grammar, including caller credentials, at most 512 bytes, matching persisted preference reads and the existing database constraint; no outbound validation occurs while editing. The worker transport's separate 4096-byte envelope is not the settings limit. SOCKS is not supported by the existing native network contract (despite the legacy frontend placeholder). Supporting it requires a separately guarded SOCKS transport and deterministic DNS/CONNECT/proxy fixtures; relaxing validation is not a substitute.
- `use_pool_cookies` changes opt-in only. `has_pool_access` comes from current `dl.shared_cookies` authority, independent of opt-in. Existing profile selection checks both.
- `/settings`, `/split`, `/keyboard`, `/mediainfo`, `/subs` expose closed revisioned controls. Split presets are exactly 524288000, 1073741824, 1610612736, 2043000000 bytes. Layouts are exactly OFF/1x3/2x3/FULL. Subtitle enabled and always-ask are explicitly labeled stored preferences, not implemented processing promises.
- `/clean` offers revisioned selective groups and all. Format resets quality/codec/container only. Language resets core language to en in the same Immediate transaction. Proxy resets enabled only; split resets 2043000000; subtitles reset enabled/auto/always-ask false and language en; NSFW resets blur true; media-info false; keyboard 2x3; arguments `{}`. All is exactly their union. Credentials, cookies, send-as-file, audio codec, gallery ZIP, pool policy, auth mode and forwarding settings survive. Overflow revisions conflict rather than becoming floating point.

## Actual worker/delivery behavior

- `/link [best|720|1080|480|360|audio] URL` admits a separate `Links`/`AudioLinks` intent through existing permissions, quota/idempotency/network-reference machinery. It is actor-private, metadata-only and bypasses file-ID cache reads/publication. It does not replace `/list` or the accepted quality action picker.
- The worker uses yt-dlp `extract_info(download=False)` with no media postprocessors, playlist expansion or download call. Extracted stream URLs pass existing network guards too. Terminal output is a confined `links.json`, **not** a format report. It contains only optional video/audio URLs, at most 1800 ASCII bytes each and 4096 encoded bytes total; at least one is required. Credential-bearing output is redacted in Debug and never placed in operation state or cache.
- Executor validates exact report name/MIME, bounded JSON and URL grammar using the same no-follow, regular-file, single-hardlink, size-checked open-descriptor confinement as other artifacts. A format report cannot be used as a stream report. Telegram sends the URLs as plain text with link previews disabled, only to the authorized private destination.
- `DownloadParams.media_info` is a frozen boolean in persisted request JSON and worker options. Admission reads the preference only for actor-private video/audio/image requests. Replay ignores this server-derived preference when comparing intent, but the admitted snapshot remains immutable.
- Worker reports use the already guarded **local** ffprobe, not a remote URL or custom command. The typed report is closed technical data: file size and at most 16 stream kind/codec/dimension records. Arbitrary tags, file paths, titles and credentials are not included. Executor validates bounds and only retains reports for opted-in, exactly-one-artifact media jobs. Telegram emits a best-effort report only after a confirmed private single-file upload receipt is persisted. Diagnostic delivery stays within the supervised service lifecycle, with the configured delivery timeout and shutdown cancellation, separate from upload receipt delivery; a stalled, cancelled or failed report never invalidates the upload receipt. Cached file IDs have no local report. This is a compact native probe report, not a claim of byte-identical legacy `mediainfo` HTML.

## SQL/models/migration proposal (no global revision reservation)

No new module tables or global migration numbers are required: imported `dl_user_settings` already contains all editable/reset fields. Matching Rust models are `SettingField`, `ResetSettings`, `UserSettingsPatch`, `UserSettings`; module SQL is `settings_{read,write,reset,reset_language}.sql`, `user_settings_patch.sql`, `media_info_preference.sql`. SQL mutations preserve all columns outside the closed selected set; language uses the existing `users` model.

The runtime request codec adds `params.media_info` with a backward-decoding default false, plus separate `links` and `audio_links` modes. Old JSON need not be rewritten. Local diagnostics are explicitly excluded from the v1 media cache key, preserving pre-existing keys. New worker descriptors have optional typed `media_info`; old descriptors decode unchanged. Deploy protocol/codec/worker changes together: older workers reject unknown options, and older native binaries cannot decode new link-mode jobs. Drain new-mode operations before rollback. Do not reserve a central schema revision for these additive request contracts.

## Isolated integration hunks

Coordinator integration is confined to:

1. worker-protocol module exports, additive options/modes/descriptors;
2. storage codec, replay-intent normalization and cache-key diagnostic exclusion;
3. executor report-type validation and optional report retention;
4. HTTP module registration/OnceLock installation and the single yoink-downloads manifest/lock dependency;
5. runtime installation of `UserSettingsService` immediately after identity/database verification (not inside media startup).

Source snapshots are unchanged. No forwarding or other product module changes are included.

## Checks

Use `backend/justfile`: `test-downloader-settings`, `check-downloader-settings`, `format-downloader-settings`, and `test-downloader-settings-worker`. Native checks use the single worktree target after resource preflight. Worker checks source-mount the existing image read-only with network none; Python never runs on the host. The worker fixture's Generic extractor talks only to loopback and does not download a complete media body. Native service fixtures use a deterministic Node worker and disposable databases. No shared Docker builds, live provider requests, production writes or credentials are required.
