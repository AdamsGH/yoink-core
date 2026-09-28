# Downloader compatibility

## Boundary

The Rust Telegram adapter remains experimental and opt-in. Private commands are owner-only. A separate `YOINK_RUST_TELEGRAM_GROUPS_ENABLED=true` setting enables media commands for known, authorized users in registered groups and topics. Production Python services and PostgreSQL data are unchanged. Engine support, a Telegram command, and verified end-to-end behavior are separate milestones.

The existing [media fixture](../../backend/scripts/test-media.mjs) already exercises real container processing, native video delivery, crash/retry/cancellation handling, file-ID cache reuse and recovery, versioned cookies/proxies, revocation and credential redaction. These are not missing merely because the legacy settings UI has not been ported.

## Compatibility matrix

| Area | Rust implementation | Remaining compatibility work |
| --- | --- | --- |
| Commands | `/download`, `/job` and `/cancel` are established. Immediate commands include `/video`, `/audio`, `/image`, `/playlist`, inline `/cut` and standalone `/subs`, including own-bot suffix handling, safe validation and stable message-based idempotency. | Settings menus, callbacks, interactive conversations and the complete plain-URL flow. |
| Formats | Telegram submission freezes saved mode-specific format settings and explicit per-request flags into effective parameters. AAC/default audio uses M4A for native sending. Caller override intent is persisted separately for replay, while cache identity uses effective output parameters. | Settings menus and interactive quality selection. Provider-specific format fallback behavior is not claimed to match the legacy downloader. |
| Adaptive streams | Native fixtures download HLS MPEG-TS segments and separate DASH audio/video streams, then verify real video/audio output, replay and cached delivery after restart. Missing fragments fail before delivery. | Live streams, encrypted-stream success, DRM and provider-specific adaptive formats are not established by these fixtures. |
| Clips | Worker FFmpeg clipping and bounded clip lists exist. Inline commands construct validated clip requests. | Interactive `/cut` state, prompts, cancellation and full legacy conversational behavior. |
| Playlists | Signed, bounded ranges retain extraction order through Telegram delivery, including reverse selections. The default is the first 20 entries. Real generic-extractor fixtures verify albums, cache reuse/recovery and durable confirmed prefixes. | External-provider behavior and provider-specific format selection. Deterministic HTML fixtures do not establish live-site compatibility. |
| Galleries | `/image` selects the gallery engine. The pinned Imgbox extractor reads synthetic multi-page galleries through verified local TLS. Native acceptance covers ordered photo/document albums, nine photos plus ZIP, archive contents, cache recovery, source limits and partial delivery surviving restart. | Live-provider compatibility, other extractor families and legacy gallery settings UI. Synthetic provider pages do not establish live-site support. |
| Cache | Actor, bot, request parameters and credential versions partition cache identity. Native acceptance verifies file-ID reuse, invalid-ID recovery, expiry and disabled-cache behavior. | User cache-management commands and any legacy policy/UI differences. |
| Cookies/proxies | Read-only legacy profile resolution freezes versioned references at admission. Execution and delivery recheck authorization and current credentials. Acceptance covers authenticated fixture networking and revocation. | Cookie upload/delete/token/settings workflows and provider-specific authentication compatibility. |
| Progress | Confirmed admission replies become durable EN/RU status messages. Worker percentages, stage and confirmed file counts are edited with restart recovery, rate-limit handling and terminal acknowledgement. `/job` uses the same rendering. | Legacy progress presentation and interactive controls. A lost initial send acknowledgement cannot be recovered safely and falls back to `/job`. |
| Permissions | Groups require a separate opt-in. Global bans, group enablement, topic policy and local roles apply at admission, execution, delivery and progress. Media and replies retain the original chat/topic. `/job` and `/cancel` also require the original actor, bot, chat and topic. | New-user/group registration, non-owner private commands, private-delivery redirects, forwarding and legacy group settings workflows. |
| Subtitles | `/subs URL` freezes saved language/automatic preferences or explicit flags and sends bounded UTF-8 SRT/VTT documents. Native fixtures verify extraction, cache isolation/recovery and rejection without partial delivery. | Settings menus, embedding/burn-in, ASS/SSA fallback and live-provider coverage. Automatic fallback is tested with real yt-dlp selection over synthetic metadata, not a live provider. |

## Command contract

- `/download URL` retains its single-video grammar and does not accept flags. It now inherits saved video settings at admission.
- `/video URL`, `/audio URL` and `/image URL` select video, audio and gallery-image modes.
- `/subs URL` sends standalone SRT/VTT documents without downloading video. It accepts only `--subtitle-language CODE` and `--subtitle-auto true|false`. Bare `/subs` returns usage rather than opening the legacy settings menu. This command does not embed or burn subtitles into video.
- `/playlist URL` requests entries 1 through 20. `/playlist N URL` requests 1 through N. `/audio N URL` and `/video N URL` request only entry N. Explicit `A-B` preserves order, and `-N` selects the final N entries. A range before the URL is always a playlist selector, even when the URL has a `t`/`start` query parameter. Domain/protocol ranges are bounded to 100 entries, while configured worker item/byte budgets may be tighter.
- `/cut URL START END` is an immediate request, not the legacy interactive conversation. Unsigned decimal seconds and colon-form times are accepted. A numeric END smaller than START is a duration, otherwise it is absolute. A colon-form END is always absolute. For `/video` and `/audio`, trailing `URL START DURATION` or `URL?t=START DURATION` arguments instead use numeric durations. Parser tests and EN/RU help distinguish these forms.
- Unknown or malformed arguments must not create operations. Source URLs and credential-bearing query values stay out of debug output and error replies.

## Saved settings and overrides

Saved video settings apply quality, video codec, container and file delivery. Audio mode applies audio codec and file delivery, ignoring video-only settings. Image mode applies ZIP and file-delivery preferences. Subtitle mode applies only `subs_lang` and `subs_auto`. An explicit `/subs` works even when the embedding preference `subs_enabled` is false. Subtitle preferences do not affect other media modes. Keyboard and raw `args_json` settings are not forwarded to the worker. Cookie/proxy authorization remains separate and is still checked.

Explicit flags are mode-specific:

| Flag | Values | Modes |
| --- | --- | --- |
| `--height` | `best` or integer 1–100000 | Video, playlist, cut |
| `--video-codec` | `h264`, `h265`, `av1`, `vp9`, `any` | Video, playlist, cut |
| `--audio-codec` | `best`, `aac`, `mp3`, `opus` | Video, audio, playlist, cut |
| `--container` | `mp4`, `mkv`, `webm` | Video, playlist, cut |
| `--send-as-file` | `true`, `false` | Video, audio, image, playlist, cut |
| `--gallery-zip` | `true`, `false` | Image only |
| `--subtitle-language` | Exact language tag, such as `en`, `ru`, `zh-Hans` | Subs only |
| `--subtitle-auto` | `true`, `false` | Subs only |

Absent flags inherit relevant saved settings. Explicit `best` and `false` override saved values rather than meaning “inherit”. Duplicate, unknown, missing-value and mode-inappropriate flags are rejected. WebM requires VP9/AV1 video and explicit Opus audio. Stored `ask` requires interactive selection, which is not yet implemented, so video admission rejects it unless `--height` supplies a concrete choice or `best`. Stored audio preferences do not change video audio encoding unless an explicit `--audio-codec` is supplied.

For example, `/video --height best --send-as-file false URL` overrides a saved height limit and document delivery. `/audio --audio-codec aac URL` overrides the saved audio codec. Settings changes after admission do not alter queued processing, cache identity or same-message replay. Changing flags or other request intent under the same message ID is a conflict, not a second operation. Replay still requires current access authorization. Execution and delivery also revalidate network credentials.

Subtitle language tags contain two or three lowercase ASCII letters and at most one two-to-eight-character alphanumeric suffix. Regexes, comma lists and wildcard selection are not accepted. Track selection keeps the legacy `en`-only rule, while other languages also request their exact `-orig` track. Manual tracks take priority, and automatic tracks are eligible only when enabled. Outputs prefer SRT over VTT and must contain valid UTF-8 timing cues. At most two files are returned, each bounded by both the configured artifact limit and a 4 MiB text ceiling. The aggregate byte/item budgets still apply. Unsupported formats, absent tracks or any failed track abort before delivery. Existing non-subtitle serialized requests and cache identities remain unchanged. Older binaries cannot decode stored subtitle jobs. Keep runtime and worker revisions matched, and verify rollback compatibility separately.

## Group and topic boundary

Groups stay disabled by default. Opting in does not create users or groups, change roles, or open private commands to non-owners. Identity and group records must already exist. Bot senders, anonymous `sender_chat` messages and channel posts are ignored. `/status` remains owner-private. Unauthorized group commands are silent and create no operation.

Global bans take precedence over local grants. Group enablement and topic rules apply even to the owner. An explicit group role override wins over an auto-grant. Auto-grants can elevate the effective group role without writing the global user role. Authorization is read again before execution, delivery and progress edits. Disabling groups also blocks persisted group jobs from starting after restart.

The admission reply, individual uploads, albums and cached sends retain the source chat and topic. There is no fallback into the actor's private chat or the group's General topic. A media receipt naming another topic is unconfirmed and becomes `delivery_unknown`, with no automatic resend. `/job` and `/cancel` from another topic cannot expose or change the operation, even for its original actor. Reply-message IDs may differ on subsequent commands, but the actor, bot, chat and topic must match.

## Progress-message lifecycle

Migration v5 adds one progress notice per operation. The first confirmed admission-reply message ID wins, including command replay. Startup resumes edits of known messages only. An initial `sendMessage` whose acknowledgement is lost, or a crash before its ID is persisted, cannot be retried without risking duplicate messages. That operation remains accessible through `/job`; no replacement status message is sent automatically.

Each configured tick processes one due notice, rechecks current authorization and destination, and renders the latest operation state. Unchanged text does not issue an API request. Completed, failed, cancelled, partial and uncertain deliveries stop after an acknowledged final edit. Partial and uncertain outcomes warn against blindly resubmitting. Counts describe confirmed receipts, not proof that an unacknowledged file was never received.

Edits have a bounded deadline. Network ambiguity and unknown API failures retry the same known message. Telegram `message is not modified` counts as acknowledgement, covering a lost response followed by restart. `retry_after` is persisted, with a maximum supported delay of one day; larger delays retire the notice rather than retrying too soon. Deleted/inaccessible messages and revoked access also retire the notice without creating another message. None of these edit retries can restart media delivery. The existing no-retry rule for ambiguous media sends is unchanged.

## Verification boundary

Worker tests run only in Docker via `just test-worker`. The real command scenarios live in [command-scenarios.mjs](../../backend/scripts/media-fixture/command-scenarios.mjs) and run through `just verify-media`, not a mocked Rust submission service. They inspect uploaded video/audio/clip streams and duration, immutable request parameters, replay behavior and denied/malformed requests. The command-only playlist and gallery cases cover admission and persisted parameters. Separate [batch scenarios](../../backend/scripts/media-fixture/batch-scenarios.mjs) exercise real generic HTML playlists and gallery-dl direct images through the unchanged production worker. They verify ordered album bytes, two-message receipts, whole-album cache reuse and invalid-ID recovery, incomplete API receipts, source-batch failure before sending, and an 11-item batch whose confirmed ten-item prefix survives a rejected final send, restart and replay. No live external provider is contacted. The [preference scenarios](../../backend/scripts/media-fixture/preference-scenarios.mjs) assert actual encoded height, audio codec and native/document sending, explicit `best`/`false`, admission snapshots surviving later settings changes, replay conflicts and cache-key separation.

The [gallery scenarios](../../backend/scripts/media-fixture/gallery-scenarios.mjs) use the installed Imgbox extractor without replacing its implementation. Disposable DNS pins `imgbox.com` to a documentation address. An authenticated fixture proxy maps only that address and port to the loopback HTTPS source. A one-day fixture certificate replaces trust only in the disposable test container. Without that trust mount, extraction fails before any HTTP request or media send. The successful cases compare distinct sub-1-KiB PNGs in photo/document albums, inspect all 12 ZIP members and hashes, and verify cache reuse and invalid-ID recovery. Valid small files remain cacheable under the same upper limits as uploads. A rejected ZIP preserves nine confirmed photos across restart and replay. Missing source images and the 20-item worker bound fail without sending a partial album. `just test-media gallery` runs this subset against the built images. `just verify-media` still runs every scenario.

The [adaptive scenarios](../../backend/scripts/media-fixture/adaptive-scenarios.mjs) serve generated three-second HLS MPEG-TS and separate-stream DASH through the unchanged extractors. They verify required segment requests, output stream types/duration, audio extraction, replay and restart cache reuse. Missing middle segments, redirects to a forbidden private destination and an HLS key on a forbidden private origin fail without delivery units. yt-dlp cannot skip unavailable fragments. FFmpeg may demux already-downloaded local MPEG-TS, but its protocols remain `file,pipe` and HLS/DASH/concat demuxers remain forbidden. `just test-media adaptive` runs this subset after rebuilding the images. These are finite synthetic VOD fixtures, not live-provider, DRM or encrypted-stream success coverage.

The [subtitle scenarios](../../backend/scripts/media-fixture/subtitle-scenarios.mjs) exercise the unchanged generic extractor against local HTML track markup. Native acceptance compares SRT/VTT bytes and document albums, checks saved and explicit preference snapshots, language/automatic cache separation, invalid-ID recovery and restart reuse, and confirms no video GET occurs. Missing original tracks, invalid text, a chunked oversized body and a redirect to a prohibited private destination fail without delivery units. `just test-media subtitles` runs the focused subset against rebuilt runtime and fixture images. Container unit tests separately cover real yt-dlp manual/automatic selection and exact language matching over synthetic metadata. These checks do not establish live YouTube subtitle compatibility.

The [progress scenarios](../../backend/scripts/media-fixture/progress-scenarios.mjs) hold a real HTTP download mid-body and observe worker percentages in actual Telegram edits. They cover unchanged-text coalescing, `429` delay, a lost terminal-edit acknowledgement followed by process restart and `message is not modified`, no duplicate media or status messages, deleted-message retirement, EN/RU and authorization revocation. The test database and Bot API are disposable fixtures.

The [group scenarios](../../backend/scripts/media-fixture/group-scenarios.mjs) verify opt-in and silent denials, group-local roles without global mutation, original reply/topic routing, cached delivery to another authorized topic, albums, scoped job/cancel commands, topic revocation during a real download, and restart/replay after a wrong-topic media receipt. Native service tests also revoke group, topic or configuration access while a job is queued and assert that no worker starts.

The immediate-command slice is verified by `just verify`, `just test-worker` and `just verify-media` in `backend`, plus the root `just verify` gate. Production deployment, live provider coverage, adaptive-streaming compatibility and cutover remain outside this slice.

## Source map

- Legacy registration and commands: `plugins/yoink-dl/src/yoink_dl/plugin.py` and its `commands/` directory. Legacy clip semantics: `plugins/yoink-dl/src/yoink_dl/url/clip.py`.
- Rust adapter and parser: `backend/crates/yoink-telegram/src/lib.rs` and `media_command.rs`.
- Request invariants: `backend/crates/yoink-domain/src/lib.rs` and `backend/crates/yoink-worker-protocol/src/lib.rs`.
- Profile and cache behavior: `backend/crates/yoink-downloads/src/profile/` and `cache.rs`.
- Worker validation, processing and networking: `backend/worker/main.py`, `media_engine.py`, `subtitle_engine.py`, `network_guard.py` and `proxy_tunnel.py`.
