# Inbox

Inbox saves content, classifies saved items, and organizes GitHub Stars. Its HTTP API mounts at `/api/v1/inbox`.

The Mini App provides item views, categories, rules, folders, Stars, and team interfaces. Availability depends on current feature access and configured providers.

## Workflow

1. Save content through an authorized ingestion path.
2. Review the saved item and its classification.
3. Organize items or Stars in local folders.
4. Request an explicit remote action when GitHub changes are required.

Native durable jobs run ingestion and classification. The active deployment does not use ARQ or the retained Python JobQueue.

## Access

Local folder operations use `inbox:ingest`. Team operations use `inbox:share`. Other operations enforce their own current grants and ownership checks.

A GitHub token does not grant access to another user's saved content. The service rechecks authority around external requests and protected writes.

## API details

Folder membership uses `POST /api/v1/inbox/folders/{id}/stars` and `DELETE /api/v1/inbox/folders/{id}/stars/{star}`.

Rule testing uses `POST /api/v1/inbox/rules/{id}/test`.

The [native router](../../../backend/crates/yoink-inbox/src/http.rs) defines the complete route surface.

## Remote effects

Local folder edits do not silently rename or delete remote GitHub lists. Remote actions have explicit endpoints and durable intent.

Remote folder creation requires a stable `Idempotency-Key`. Reuse that key after response loss. An unknown creation result must not trigger another automatic create.

See [durable contracts](contracts.md), [runtime configuration](runtime.md), [Telegram delivery](telegram.md), and [content processing](content.md).
