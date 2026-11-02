# Downloader settings

Personal settings are available through the Mini App and supported private Telegram controls. The API reads and writes them at `/api/v1/dl/settings`.

The service checks the current actor and ban state. Pool access comes from current authority, not a submitted boolean.

## Supported values

| Control | Values |
| --- | --- |
| Quality | `best`, `2160`, `1440`, `1080`, `720`, `480`, `360`, `240`, `144`, `ask`. |
| Video codec | `avc1`, `vp09`, `av01`. |
| Container | `mp4`, `mkv`. |
| Audio codec | `best`, `mp4a`, `opus`, `mp3`. |
| Split presets, bytes | `524288000`, `1073741824`, `1610612736`, `2043000000`. |
| Subtitle language | `en`, `ru`, `de`, `fr`, `es`, `zh-Hans`. |
| Keyboard layout | `OFF`, `1x3`, `2x3`, `FULL`. |
| YouTube authentication | `cookies`, `oauth`, `oauth2`. |

The [typed settings parser](../../../backend/crates/yoink-downloads/src/settings.rs) defines value validation. The [PATCH model](../../../backend/crates/yoink-downloads/src/user_settings.rs) defines writable fields.

## Concurrent edits

GET and successful PATCH responses include an ETag. The Mini App sends the last revision in `If-Match` and submits only changed writable fields.

A stale revision returns HTTP 409. The settings form keeps the draft and requires explicit reload before another save. Reload discards that draft and reads the current server values.

Successful saves reset the form from the canonical response. Read-only metadata such as `user_id`, `updated_at`, and `has_pool_access` never enters the PATCH body.

The Cookies page uses the same revision contract when changing shared-pool opt-in.

## Proxy settings

Omitting `proxy_url` preserves the saved URL. Explicit `proxy_url: null` removes it.

The saved URL supports the guarded HTTP(S) proxy grammar. SOCKS URLs are not accepted by this personal setting. Editing a URL does not probe its reachability.

Automatic system-proxy domain routing is independent of the personal toggle. Disabling a personal proxy does not disable operator routing.

## Presentation and subtitles

`nsfw_blur` controls eligible private Telegram spoilers. Audio and documents do not support that spoiler behavior.

`mediainfo` requests a bounded technical report after a confirmed private single-file upload. Cache hits have no local artifact report.

Subtitle preferences do not imply automatic embedding or burn-in. The standalone subtitle command has its own extraction contract.

## Compatibility and rollback

Some settings and commands add persisted request variants without changing the database schema. Older binaries or workers can reject those variants.

Upgrade the runtime and worker together. Review [persistence compatibility](../../architecture/persistence.md) before rollback.
