# Downloader compatibility

## Boundary

The Rust Telegram adapter remains experimental, opt-in, owner-only and private-chat-only. Production Python services and PostgreSQL data are unchanged. Engine support, a Telegram command, and verified end-to-end behavior are separate milestones.

The existing [media fixture](../../backend/scripts/test-media.mjs) already exercises real container processing, native video delivery, crash/retry/cancellation handling, file-ID cache reuse and recovery, versioned cookies/proxies, revocation and credential redaction. These are not missing merely because the legacy settings UI has not been ported.

## Compatibility matrix

| Area | Rust implementation | Remaining compatibility work |
| --- | --- | --- |
| Commands | `/download`, `/job` and `/cancel` are established. The current slice adds `/video`, `/audio`, `/image`, `/playlist` and inline `/cut`, including own-bot suffix handling, safe validation and stable message-based idempotency. | Settings menus, callbacks, interactive conversations and the complete plain-URL flow. |
| Formats | Telegram submission freezes saved mode-specific format settings and explicit per-request flags into effective parameters. AAC/default audio uses M4A for native sending. Caller override intent is persisted separately for replay, while cache identity uses effective output parameters. | Settings menus and interactive quality selection. Provider-specific format fallback behavior is not claimed to match the legacy downloader. |
| Clips | Worker FFmpeg clipping and bounded clip lists exist. Inline commands construct validated clip requests. | Interactive `/cut` state, prompts, cancellation and full legacy conversational behavior. |
| Playlists | Signed, bounded ranges retain their order across the Rust request and Python protocol. The playlist command defaults to the first 20 entries. | Real multi-entry provider-to-Telegram acceptance, including album/prefix failure cases. Command admission is not proof of provider compatibility. |
| Galleries | `/image` explicitly selects the gallery engine. Container tests exercise the pinned gallery-dl download/write path, ordering, partial failure, limits and ZIP generation. | Gallery command through actual native photo/album delivery. Persisted command parameters alone do not establish this. |
| Cache | Actor, bot, request parameters and credential versions partition cache identity. Native acceptance verifies file-ID reuse, invalid-ID recovery, expiry and disabled-cache behavior. | User cache-management commands and any legacy policy/UI differences. |
| Cookies/proxies | Read-only legacy profile resolution freezes versioned references at admission. Execution and delivery recheck authorization and current credentials. Acceptance covers authenticated fixture networking and revocation. | Cookie upload/delete/token/settings workflows and provider-specific authentication compatibility. |
| Progress | Worker progress events and durable operation state exist. `/job` reads status. | User-facing progress updates and terminal message lifecycle. A worker event is not a Telegram progress update. |
| Permissions | Owner-private command gate and private-scope identity checks apply at admission, execution and delivery. Group/non-owner commands are denied. | Group/topic authorization and delivery routing, and non-owner product access. No group parity is claimed. |
| Subtitles | No Rust request/worker option or command yet. | Saved preferences, language selection, automatic subtitles and standalone subtitle delivery. |

## Command contract

- `/download URL` retains its single-video grammar and does not accept flags. It now inherits saved video settings at admission.
- `/video URL`, `/audio URL` and `/image URL` select video, audio and gallery-image modes.
- `/playlist URL` requests entries 1 through 20. `/playlist N URL` requests 1 through N. `/audio N URL` and `/video N URL` request only entry N. Explicit `A-B` preserves order, and `-N` selects the final N entries. A range before the URL is always a playlist selector, even when the URL has a `t`/`start` query parameter. Domain/protocol ranges are bounded to 100 entries, while configured worker item/byte budgets may be tighter.
- `/cut URL START END` is an immediate request, not the legacy interactive conversation. Unsigned decimal seconds and colon-form times are accepted. A numeric END smaller than START is a duration, otherwise it is absolute. A colon-form END is always absolute. For `/video` and `/audio`, trailing `URL START DURATION` or `URL?t=START DURATION` arguments instead use numeric durations. Parser tests and EN/RU help distinguish these forms.
- Unknown or malformed arguments must not create operations. Source URLs and credential-bearing query values stay out of debug output and error replies.

## Saved settings and overrides

Saved video settings apply quality, video codec, container and file delivery. Audio mode applies audio codec and file delivery, ignoring video-only settings. Image mode applies ZIP and file-delivery preferences. Unrelated subtitle, keyboard and raw `args_json` settings are not forwarded to the worker. Cookie/proxy authorization remains separate and is still checked.

Explicit flags are available on `/video`, `/audio`, `/image`, `/playlist` and inline `/cut`:

| Flag | Values | Modes |
| --- | --- | --- |
| `--height` | `best` or integer 1–100000 | Video, playlist, cut |
| `--video-codec` | `h264`, `h265`, `av1`, `vp9`, `any` | Video, playlist, cut |
| `--audio-codec` | `best`, `aac`, `mp3`, `opus` | Video, audio, playlist, cut |
| `--container` | `mp4`, `mkv`, `webm` | Video, playlist, cut |
| `--send-as-file` | `true`, `false` | All media commands |
| `--gallery-zip` | `true`, `false` | Image only |

Absent flags inherit relevant saved settings. Explicit `best` and `false` override saved values rather than meaning “inherit”. Duplicate, unknown, missing-value and mode-inappropriate flags are rejected. WebM requires VP9/AV1 video and explicit Opus audio. Stored `ask` requires interactive selection, which is not yet implemented, so video admission rejects it unless `--height` supplies a concrete choice or `best`. Stored audio preferences do not change video audio encoding unless an explicit `--audio-codec` is supplied.

For example, `/video --height best --send-as-file false URL` overrides a saved height limit and document delivery. `/audio --audio-codec aac URL` overrides the saved audio codec. Settings changes after admission do not alter queued processing, cache identity or same-message replay. Changing flags or other request intent under the same message ID is a conflict, not a second operation. Replay still requires current access authorization. Execution and delivery also revalidate network credentials.

## Verification boundary

Worker tests run only in Docker via `just test-worker`. The real command scenarios live in [command-scenarios.mjs](../../backend/scripts/media-fixture/command-scenarios.mjs) and run through `just verify-media`, not a mocked Rust submission service. They inspect uploaded video/audio/clip streams and duration, immutable request parameters, replay behavior and denied/malformed requests. Playlist and gallery scenarios currently cover admission and persisted parameters only. The [preference scenarios](../../backend/scripts/media-fixture/preference-scenarios.mjs) assert actual encoded height, audio codec and native/document sending, explicit `best`/`false`, admission snapshots surviving later settings changes, replay conflicts and cache-key separation.

The immediate-command slice is verified by `just verify`, `just test-worker` and `just verify-media` in `backend`, plus the root `just verify` gate. Production deployment, live provider coverage, adaptive-streaming compatibility and cutover remain outside this slice.

## Source map

- Legacy registration and commands: `plugins/yoink-dl/src/yoink_dl/plugin.py` and its `commands/` directory. Legacy clip semantics: `plugins/yoink-dl/src/yoink_dl/url/clip.py`.
- Rust adapter and parser: `backend/crates/yoink-telegram/src/lib.rs` and `media_command.rs`.
- Request invariants: `backend/crates/yoink-domain/src/lib.rs` and `backend/crates/yoink-worker-protocol/src/lib.rs`.
- Profile and cache behavior: `backend/crates/yoink-downloads/src/profile/` and `cache.rs`.
- Worker validation, processing and networking: `backend/worker/main.py`, `media_engine.py`, `network_guard.py` and `proxy_tunnel.py`.
