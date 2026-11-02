# Downloader commands and compatibility

The supplied deployment uses the native Rust downloader. Standalone Telegram and media adapters remain disabled until explicitly configured.

Engine support, an exposed command, and successful live-provider delivery are separate facts. The native worker supports more than a single progressive video path, but it does not promise complete legacy parity.

## Command contract

| Command | Behavior |
| --- | --- |
| `/download URL` | Single video with saved preferences. No flags. |
| `/video URL` | Video download. |
| `/audio URL` | Audio extraction. |
| `/image URL` | Gallery/image extraction. |
| `/playlist URL` | Entries 1 through 20 by default. |
| `/list URL` | Bounded source format report without media download. |
| `/link URL` | Private stream-link report without media download. |
| `/subs URL` | Standalone subtitle documents. |
| `/cut URL START END` | Immediate clip request. |
| `/job UUID`, `/cancel UUID` | Caller-owned operation inspection or cancellation. |

Unknown or malformed arguments create no operation. Credentials and source query values stay out of public errors.

## Playlists and clips

`/playlist N URL` selects entries 1 through N. `/video N URL` and `/audio N URL` select entry N.

An explicit `A-B` range preserves order. `-N` selects the final N entries. Protocol ranges allow at most 100 entries. Configured item and byte limits can be tighter.

A range before the URL is a playlist selector, not a timestamp.

For immediate `/cut`, a numeric END smaller than START means duration. Otherwise it is absolute. A colon-form END is always absolute.

For trailing `URL START DURATION` on video/audio commands, a numeric final value means duration. These grammars are intentionally distinct.

`/cut URL` and `/cut URL START` open the private clip dialog. Confirmation freezes the selected ranges and quality.

## Plain URL messages

A visible Telegram URL entity can follow the video path. Hidden text-link targets, captions, and content inserted through another bot do not trigger implicit downloading.

In groups, the URL must reply to this bot or originate through its inline mode. Normal group/topic authorization still applies.

Supported timestamp suffixes can open or complete a private clip workflow. Active clip input takes precedence. Use `/cancel` before starting an unrelated implicit download.

Music domains remain reserved for Music. An inline result does not bypass destination checks.

## Per-download quality choice

A saved quality of `ask` opens a durable private chooser for a ready video request. Explicit height overrides avoid the prompt.

The chooser supports height, Audio, and Cut where eligible. Cut transfers a single-video request into the durable clip editor without admitting the full source.

Stale or replaced callbacks cannot create a second operation. Restart preserves the linked draft. A linked quality menu resumes the existing clip editor instead of restarting the source.

Groups and direct HTTP admission require concrete quality rather than an interactive private prompt.

## Saved settings and overrides

Saved video preferences apply height, codec, container, and file delivery. Audio uses its audio preference and ignores video-only fields.

Image requests use gallery ZIP and file-delivery preferences. Subtitle requests use subtitle language and automatic-track selection.

Explicit command overrides differ from stored settings:

| Flag | Accepted values |
| --- | --- |
| `--height` | `best` or integer 1 through 100000. |
| `--video-codec` | `h264`, `h265`, `av1`, `vp9`, `any`. |
| `--audio-codec` | `best`, `aac`, `mp3`, `opus`. |
| `--container` | `mp4`, `mkv`, `webm`. |
| `--send-as-file` | `true` or `false`. |
| `--gallery-zip` | `true` or `false`, image mode only. |
| `--subtitle-language` | Exact supported language tag. |
| `--subtitle-auto` | `true` or `false`, subtitle mode only. |

Mode-inappropriate, duplicate, unknown, and incomplete flags fail validation. WebM requires compatible video and explicit Opus audio.

Explicit `best` and `false` override saved values. They do not mean inheritance.

Settings changes after admission cannot change an immutable queued request or its cache identity. Changed intent under the same message ID conflicts.

See [settings](settings.md) for stored values, ETags, clearing a proxy, and presentation controls.

## Subtitle preferences

Bare `/subs` opens the private language and automatic-track menu. `/subs URL` works independently of the stored embedding preference.

The worker prefers SRT, then VTT, then ASS, then SSA. It returns at most two validated UTF-8 text documents.

Language selection is exact, not a regular expression or wildcard. Manual tracks take priority. Automatic tracks require explicit permission.

ASS/SSA remain text documents. The worker does not render tags, load fonts, embed subtitles, or burn them into video.

## Quotas and active work

Minute, hour, and day quotas default to 5, 30, and 100 accepted operations. Windows begin at their first accepted operation.

Quota rows survive restart. Only the exact configured owner bypasses time quotas.

Charging commits with new admission. Replay, conflicts, invalid requests, and rejected capacity do not consume quota. Worker retries do not charge again.

A separate active-work limit defaults to three running or delivering operations per actor. It includes the owner and cache hits.

Pending queue capacity remains separate. See [configuration](../../operations/configuration.md) for native setting ownership.

## Personal cookies and proxy usage

Personal cookie operations require the actor's own authorized private context. Shared pool administration has separate authority.

Document and URL imports validate Netscape data and enforce capacity. Replacing or deleting a selected cookie invalidates frozen references.

Cookie URL import uses a separate direct guarded client. It rejects private destinations by default, HTTPS downgrade, invalid encoding, and bodies above 512 KiB.

`/proxy` toggles use of existing proxy configuration. It does not accept or reveal proxy credentials. Stale callbacks cannot undo a newer revision.

Automatic system-proxy routing is independent of the personal toggle. A configured proxy failure cannot silently become direct egress.

See [cookies](cookies.md), [history and routing](history.md), and the [browser extension](../../../browser-extension/README.md).

## Group and topic boundary

Groups require explicit enablement and current registered policy. Discovery events create disabled groups rather than enabling access.

Global bans override local grants. Group enablement and topic policy also apply to the owner.

Replies, uploads, albums, and cached sends retain the original chat/topic. There is no silent redirect to the actor's private chat or General topic.

`/job` and `/cancel` require the original actor, bot, chat, and topic. A receipt for another destination remains unconfirmed.

## Progress-message lifecycle

Confirmed private success removes eligible source input and progress notices. Group source messages remain.

An input reference does not authorize editing a bot message. Unknown message IDs cannot be guessed or reconstructed.

Progress edits are separate from media sends. Safe edit retries do not authorize repeating an uncertain upload.

## Versioned automatic provider routing

New automatic requests freeze routing table v1 at admission. Explicit engine intent stays explicit.

YouTube selects yt-dlp. Selected gallery hosts select gallery first. VK wall/album paths have specific routing. Instagram can use guarded yt-dlp-to-gallery fallback.

Matching uses canonical hosts and path prefixes, not embedded URL text or query strings.

Fallback applies only to recognized extraction failures. Policy denial, cancellation, resource limits, credential setup, and local processing failures cannot authorize another provider.

Typed `auto_*_v1` values enter persisted requests and cache identity without a new schema version. Older binaries or workers can reject them.

Keep runtime and worker revisions paired. Never rewrite frozen routing or delivery journals to force rollback.

## Verification boundary

`just --justfile backend/justfile verify-media` runs the real worker path against isolated sources, Bot API, proxy, and native storage.

Focused scenarios cover quality/cut, quotas, subtitles, cookies, NSFW policy, routing, albums, cache, replay, and cancellation.

Synthetic provider responses do not establish every live provider, authentication flow, or production edge configuration.

See [media fixtures](../../development/media-fixtures.md), [inline search](inline-search.md), [content policy](nsfw.md), and [deferred media](deferred-media.md).
