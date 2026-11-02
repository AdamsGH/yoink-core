# Downloads

Send a supported media URL to the bot or use an explicit download command. The native service checks access, quotas, destination policy, and credentials before execution and delivery.

## Common commands

```text
/download URL
/video URL
/audio URL
/image URL
/playlist URL
/list URL
/link URL
/job UUID
/cancel UUID
```

`/list` returns a source format report. `/link` returns guarded stream URLs privately without downloading the media. Provider support does not guarantee that a particular URL is accessible.

Private `/audio`, `/image`, or `/video` without a URL selects the mode for the next accepted plain URL. Groups require an explicit URL. See [deferred media](deferred-media.md).

Use the [command reference](compatibility.md) for clips, playlists, subtitles, quality dialogs, overrides, and limits.

## Settings

The Mini App and private Telegram controls edit personal preferences. The [settings contract](settings.md) lists supported codecs, split sizes, proxy behavior, and concurrent-edit handling.

NSFW blur controls private photo/video spoilers. It does not bypass group content rules or add visual content detection. See [content policy](nsfw.md).

## Authentication and history

Personal cookies can enable authenticated provider access. They remain secrets and may expire or stop working. See [cookies](cookies.md) and the [browser extension](../../../browser-extension/README.md).

[Download history](history.md) combines imported logs with native operations. A retry creates a new authorized attempt using current settings.

An unknown delivery result must not be retried automatically. Check the destination before submitting another download.
