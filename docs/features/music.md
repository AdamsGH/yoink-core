# Music

Music resolves provider URLs into cross-provider cards. It supports Spotify, Deezer, Yandex, YouTube Music, YouTube, SoundCloud, Apple Music, Tidal, Qobuz, and Bandcamp links.

Provider parsing and successful remote resolution are different capabilities. Upstream APIs, region restrictions, and credentials can limit a result.

## Use

```text
/music URL
/song URL
```

Supported music URLs also work in personal inline queries. Cards use text links and current access checks.

Optional audio delivery applies to track cards, not artist cards. The confined worker produces MP3 audio with title, performer, and optional cover metadata. The service rechecks access before delivery.

## Configuration

All settings use the `YOINK_RUST_` prefix.

| Setting | Default |
| --- | --- |
| `YOINK_RUST_MUSIC_ENABLED` | `false` standalone. |
| `YOINK_RUST_MUSIC_DOWNLOAD_ENABLED` | `false`. |
| `YOINK_RUST_MUSIC_REQUEST_TIMEOUT_SECS` | `10`. |
| `YOINK_RUST_MUSIC_CACHE_TTL_SECS` | `3600`. |

Spotify and Tidal each accept native client ID and secret settings. Provider proxies use `YOINK_RUST_MUSIC_PROXY_URL` and `YOINK_RUST_MUSIC_PROXY_PLATFORMS`.

The [product configuration reference](../architecture/products.md) lists worker, response, cache, and delivery bounds. The [typed configuration](../../backend/crates/yoink-server/src/product_config.rs) defines validation.

## Persistence and delivery

Resolution activity and audio outcomes use native storage. Native audio cache identities include actor, bot, credentials, source, and metadata.

Confirmed receipts replay without another send. Unknown delivery never authorizes an automatic resend.

The native runtime does not start the retained Python music service or depend on a separate downloader plugin process.
