# Inbox changes

- Use the current shared runtime in `backend/crates/yoink-server/src/inbox_runtime.rs`. Do not apply historical integration patches.
- Share the owning database across Inbox, identity, job leases, and text delivery.
- Preserve current feature checks around external I/O and protected writes.
- Keep local folder changes separate from explicit GitHub mutations.
- Preserve stable remote-create keys and fail-closed unknown-effect recovery.
- Register schema changes through the existing checksummed native migration runner. Do not initialize tables from product startup.
- Read [Inbox contracts](../../features/inbox/contracts.md) and [Telegram delivery](../../features/inbox/telegram.md) before changing recovery.
- Use [the actual-process fixture](../../development/fixtures/inbox.md) for cross-module acceptance. Unit tests do not establish real transport behavior.
