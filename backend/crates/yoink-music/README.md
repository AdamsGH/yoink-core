# Native Music integration

`Executor` directly implements `MusicWorker`. Construct
`NativeMusicAuthority(Arc<IdentityStore>)` and one shared
`Arc<NativeMusicPersistence> { database, store }`. Pass that persistence adapter
also as `ResolveAnalytics`. Credentials/proxies come from typed native admin
configuration, never environment discovery or imported legacy runtime settings.

## Parent-owned transport wiring

- Register `/music` and `/song`. Supply trusted `MusicActor`, command/reply text
  and Telegram text-link entity URLs to `command`. Send cards using supplied
  UTF-16 entities and existing native reply/topic handling.
- After sending a track card, call `send_audio(AudioCommand { actor, target,
  track, caption }, persistence, delivery, cancel)` when downloads are enabled.
  Add the trusted requester to the caption first. Artist cards do not send audio.
- Implement `MusicAudioDelivery::send` with native Telegram `sendAudio`, title,
  performer and exact caption entities. Upload using the validated open file
  descriptor, never its deleted workspace path. `Cached` uses its file ID.
  Return a positive message ID and audio file ID. Only explicit non-acceptance
  is `DeliveryRejected`; transport errors/lost receipts are `DeliveryUnknown`.
  Never retry uncertain external sends.
- Route supported personal URL queries to `inline`, using `parse_link` first.
  Send articles with `cache_time=0`, `is_personal=true`, supplied entities and
  existing localized share/PM controls. Preserve central non-Music fallbacks for
  unsupported queries. All stages reload fresh native Music feature grants.
- Expose `stats` at authenticated `/api/v1/music/me/stats` and `activity` through
  the native account aggregator. Actor IDs come only from native authentication.
  DTO timestamps are native microseconds. Day/week boundaries use UTC and
  Monday. Use native language codes with `error_message` for EN/RU errors.

## Persistence and admission

Parent schema/importer must include `music_resolve_log` with legacy columns
`id`, `user_id`, `source_platform`, `artist`, `title`, `platforms_found`,
`created_at`. Queries use microseconds like other native tables. Resolution log
failures do not invalidate cards. Audio writes existing `download_log` with
quality `audio`, status `ok`, `cached` or `error`.

`file_cache` reuse is limited to new `music:native:` keys containing format/tag
version, trusted owner, bot, credential identity, source and full metadata via
structured serialization. Legacy keys are not reused. Fresh grants are checked
again before delivery. Settings bound worker/query/delivery/cache budgets.

Audio uses Store's existing effect journal through a Music adapter, not
`TextDelivery`. Namespaced `music:audio:` tokens bind immutable actual principal,
caption, track, target and credential identity. `item=None` and `bind_item=false`
exclude inbox recovery. CAS allows at most one external attempt. Confirmed
receipts replay authoritatively. Unknown/armed/lost receipts never authorize
another attempt. Known receipts remain confirmed if persistence later fails.

## Worker verification

Include `music_engine.py` and `music_library` in the worker image. Dependency
pins are in `backend/Dockerfile`. No host Python or Python service startup is
needed. Confined MP3 uses 192 kbps and actual ID3 title/performer/optional cover.
YouTube parsing and audio extraction require Music category. Audio tries
resolved YouTube Music/YouTube links, then bounded YouTube Music and audio-only
YouTube search. Ordinary cross-provider cards never add YouTube search results.

Native checks: `just test-crate yoink-music`, `just test-crate yoink-executor`,
`just test-crate yoink-worker-protocol` and corresponding `lint-crate` recipes.
Run `python -B -m unittest test_music_engine` only inside the rebuilt pinned
worker container. Offline fixtures cover dispatch signatures, API/HTML metadata,
category rejection, private/body/request budgets and actual MP3 ID3 tags. Live
provider success still depends on public APIs, regional access and configured
credentials. Synthetic tests are not live-provider evidence.
