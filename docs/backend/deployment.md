# Native deployment

The default tracked Compose manifest now selects the Rust runtime. Build it with
`just build yoink`; `just up` and `just restart yoink` retain the native service.
The legacy ARQ worker has the explicit `legacy` profile and must stay stopped.
PostgreSQL and its existing volumes remain available for rollback, not replication.
The native service exposes the existing port 8000 and preserves the nginx/TLS edge.

## Configuration

The shared private `.env` is still authoritative. Native settings use the
`YOINK_RUST_` prefix; legacy optional credentials must be copied to their native
names only when nonempty. See `backend/crates/yoink-server/PRODUCT_RUNTIME.md` and the typed server
configuration. Preserve the configured downloader/Music proxies, Insight gateway,
API signing key, owner and bot token. The Compose manifest explicitly enables all
installed products and authentication. Workspaces are private, separate tmpfs
mounts; the database, search index and Stats import directory live in
`data/native`, owned by UID/GID 10001. No legacy application directory is mounted. The Bot API data directory is mounted
read-only at /telegram-files with explicit remote/local roots; grant UID 10001
read/traverse access through ACLs, including default directory ACLs for new files.

The Unix YouTube gateway has a dedicated private volume and only connects to the
existing trusted youtubei/bgutil services. It has no host port. Its init service
sets socket-directory ownership before startup. The production image contains no
fixture server or refresh controls. Synthetic acceptance does not establish live
account authorization or real PoToken issuance.

## Cutover and rollback

1. Finish the matching-image acceptance and configuration checks, and commit the
   reviewed integration. Record immutable image IDs and a private configuration
   backup.
2. Stop only `yoink` and `yoink-arq-worker`, plus any independently configured
   writers. Keep PostgreSQL and existing data. Take a private PostgreSQL dump and
   run the immutable importer against the stopped-writer source. Require successful
   table/row/type digest, foreign-key and allocator verification before publication.
3. Verify a native archive restore with the operations image. Publish the imported
   database into the private native directory; do not import over a live database.
4. Start the gateway/init and then `yoink`. Never run two Telegram pollers.
   Check readiness, real database/authentication/product health, TLS routing,
   polling and the original process's shutdown behavior.

Before native writes, stop the native service and resume the untouched legacy
application with `docker compose -f docker-compose.yaml -f docker-compose.legacy.yaml up -d --no-deps yoink yoink-arq-worker`.
This explicitly selects the reviewed legacy override. Keep the nginx/TLS edge unchanged.
After native writes, **do not silently restart legacy**: its PostgreSQL snapshot is
stale. Preserve and back up native state, then use an independently verified reverse
migration or obtain an explicit data-loss decision. No reverse migration is claimed.
See [import](import.md) and [backup/restore](backup-restore.md) for operator contracts.

## Maintenance

`just build backup` builds the operator image. Each maintenance run resolves its
immutable image ID before starting the offline operation. `just backup run` drains/stops the native writer, makes a verified private archive
in `data/native-backups`, then resumes the same runtime. Check readiness after
maintenance. `just backup restore <archive.ybk>` restores only an inactive new file
in `data/native-restore`; it never switches production. Both directories must be
private and owned by UID/GID 10001. Archives are plaintext. The former PostgreSQL
cron service is retained under `legacy-backup`, not a native backup schedule.

When `yoink_rust_image` is pinned in the private `.env`, building a newer tag does
not change that pin. Verify the new image and update the pin before deployment.
Native schemas migrate/verify on startup; the former Alembic dispatcher fails
closed. Default lint/test/verify recipes check Rust; `verify-py` remains an explicit
container-only legacy reference check.
