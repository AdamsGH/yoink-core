# Native deferred private media

Authorized private `/audio`, `/image` and `/video` without arguments select a
one-shot requested mode for the next *new accepted plain URL* in that actor's
current bot/private chat. `/video` selects video explicitly, rather than exposing
an earlier audio/image arm. Immediate URL commands retain their explicit mode.
Groups require a URL and never write or read this private arm. No expiry is added.

Migration 18 adds a private actor/bot/chat watermark with a nullable pending mode,
original Telegram operation intents, and original owner-quality draft intents.
The watermark survives consumption: duplicate/older commands cannot re-arm or
replace newer commands. An older URL cannot consume a newer command. A successful
new explicit URL admission clears only obsolete intent, not a command newer than
that URL. Non-URL text never reaches admission.

The Telegram adapter supplies a trusted admission context separately from the
public request body. Before preference/network preparation, the service selects
the requested mode and uses existing download authorization at arm and submission.
The immediate storage transaction rechecks role/ban and the observed state version,
then atomically charges quotas, consumes NSFW mark, inserts the immutable resolved
worker request/cache identity and retains original Telegram intent. Failed validation,
conflict, capacity, quota, insertion or commit leaves the arm pending. State races
retry preparation, not stale mode insertion. No trusted context enters worker/cache
metadata, and no domain/worker codec options are added.

Replay checks the original raw Telegram request before pending state/preferences
are consulted. The existing ordinary admission comparison remains unchanged for
non-Telegram calls and pre-migration operations. Quality dialogs keep the original
Telegram intent durably and do not consume an arm until successful admission;
the existing owner-only callbacks, choices, cut handoff and cancel fences remain.
Superseding a quality draft removes its original unless a surviving cut still uses
that UUID. Cut cancellation or replacement removes the outgoing original only when
neither a current quality draft nor a replayable cut needs it. Both removals occur
inside the session save transaction; immutable operation originals remain intact.
NSFW's actor-wide mark remains independent of the private media scope and is
consumed by the same operation transaction. Cache behavior remains nonsticky.

## Verification

- `cd backend && just test-deferred-media`: focused native storage/admission,
  adapter and EN/RU contracts on disposable databases.
- `cd backend && just test-media deferred-media`: exact real downloader fixture,
  reusing native Telegram/queue handlers, media source, gallery source and ffprobe.
  Coordinator acceptance passed on matching runtime and fixture images, including
  real audio/image delivery, video reset, restart and immutable URL replay.

Admission retains this order: select deferred requested mode, resolve preferences,
freeze engine/network authority, compute cache identity. Original Telegram intent uses the raw caller engine; provider
snapshot normalization must not reinterpret it using current pending state.

Migration 18 requires a matching binary. Rollback must restore a compatible
pre-migration backup into a separate database and use the normal ownership handoff.
See [persistence](../../docs/backend/persistence.md).
