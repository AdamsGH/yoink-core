# Yoink Inbox

Inbox adds saved content, classification, GitHub Stars, folders, and team interfaces to Yoink.

The active service runs in the shared Rust backend. This core-owned directory contains the frontend and retained Python reference implementation. It is not a Git submodule or an independent native service.

## Runtime

Native durable jobs run ingestion, content processing, and classification. The default deployment does not use ARQ or the Python JobQueue.

Local folder operations require current `inbox:ingest` access. Team operations use `inbox:share`. The service applies additional ownership and grant checks to each operation.

## API and recovery

The API mounts at `/api/v1/inbox`. Folder membership uses `POST /folders/{id}/stars` and `DELETE /folders/{id}/stars/{star}`. Rule testing uses `POST /rules/{id}/test`.

Local folder edits do not silently modify GitHub lists. Remote creation requires a stable `Idempotency-Key`. Unknown external outcomes do not permit automatic repeated mutations.

## Documentation

- [Inbox guide](../../docs/features/inbox/README.md)
- [Durable contracts](../../docs/features/inbox/contracts.md)
- [Runtime configuration](../../docs/features/inbox/runtime.md)
- [Telegram delivery](../../docs/features/inbox/telegram.md)
- [Installation](../../docs/operations/installation.md)
