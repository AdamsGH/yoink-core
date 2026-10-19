# Native deployment

The default Compose stack has six services: the Rust backend, frontend, nginx, local Bot API, proxy facade and combined YouTube service. Build the backend with `just build yoink` and the YouTube service with `just build gateway`. PostgreSQL, Redis, ARQ and the old standalone YouTube helpers are excluded from default startup. They remain available through the legacy override for rollback; their data volumes are preserved. The native service exposes the existing port 8000 and preserves the nginx/TLS edge.

## Configuration

The shared private `.env` is still authoritative. Native settings use the
`YOINK_RUST_` prefix; legacy optional credentials must be copied to their native
names only when nonempty. See `backend/crates/yoink-server/PRODUCT_RUNTIME.md` and the typed server
configuration. Preserve the configured downloader/Music proxies, Insight gateway,
API signing key, owner and bot token. The Compose manifest explicitly enables all
installed products and authentication. Initial command-menu reconciliation is
bounded by the configured 300s startup deadline; unavailable historic users do
not keep the entire pass dirty. The 150s shutdown budget fits the 165s Docker grace.
Workspaces are private, separate tmpfs
mounts; the database, search index and Stats import directory live in
`data/native`, owned by UID/GID 10001. No legacy application directory is mounted. The Bot API data directory is mounted
read-only at /telegram-files with explicit remote/local roots; grant UID 10001
read/traverse access through ACLs, including default directory ACLs for new files.

The YouTube service packages youtubei, pinned bgutil 2.0.0 and the Unix gateway in one container. A supervisor fails the whole container if any component exits, drains the gateway before stopping providers and kills remaining process groups at the shutdown deadline. Its healthcheck probes all three processes without consuming download admission slots. Busy admission remains healthy; exhausted slots from uncertain PoToken responses fail readiness.

All processes run as UID/GID 10001 with a read-only root, bounded tmpfs and resource ceilings. The private socket directory is owned by UID 10001 in the image, so Docker initializes fresh named volumes without a separate init service. Existing socket volumes must retain that ownership. Only the Unix socket is mounted into workers; no helper HTTP port is published.

`yoink_youtube_image` selects an immutable SemVer image built from verified source snapshots; changing build inputs requires a new tag. Upstream stderr is suppressed because it can contain OAuth credentials; the supervisor logs component exits without request data. Synthetic acceptance does not establish live account authorization or real PoToken issuance.

## Source DNS

Worker source admission rejects private and benchmark addresses. When Mihomo DNS
returns only fake-IP addresses in `198.18.0.0/15` for a hostname, workers recover
real IPv4 addresses through Cloudflare DNS-over-HTTPS at `https://1.1.1.1/dns-query`.
TLS verification, bounded responses and the existing worker socket timeout apply.
Every recovered address must be public; IP-literal sources and mixed private DNS
answers remain rejected. Recovered answers are pinned for that worker operation.
Downloader/Music/Insight traffic still uses its configured proxy.

`YOINK_RUST_INSIGHT_RESPONSE_MAX_BYTES` limits source downloads and provider
responses. Its default is 1 MiB; the shared client maximum is 4 MiB. Use
`4194304` for YouTube pages that exceed the default. Larger values fail
configuration validation before startup.

Music derives trusted proxy origins from `YOINK_RUST_MUSIC_PROXY_URL` for the
configured proxy platforms. This does not authorize private source addresses.
`YOINK_RUST_MUSIC_WORKER_RESPONSE_BYTES` separately bounds upstream metadata
and extractor bodies (default 1 MiB, maximum 8 MiB). YouTube audio extraction
can require `4194304`; the MP3 artifact limit remains separate.

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

Production images use the SemVer version in `backend/VERSION`. Use the [release recipes](../../backend/README.md#releases) to build immutable tags and deploy them. Deployment updates `yoink_rust_image` in the private `.env` and retains one previous version until Adams confirms behavior. Docker health does not confirm a release. Use `release-confirm` only after that confirmation. Native schemas migrate/verify on startup; the former Alembic dispatcher fails closed. Default lint/test/verify recipes check Rust; `verify-py` remains an explicit container-only legacy reference check.

## Verified production cutover

The 2026-10-09 cutover stopped the legacy app and ARQ writers before the final
snapshot. The pinned importer verified 42 source tables and 13,583 rows, including
typed digests, foreign keys and allocators. Independent native archive/restore
comparison verified the schema and all typed rows across 103 tables. PostgreSQL,
Redis and private configuration snapshots remain retained for recovery.

The historical cutover image was `sha256:614fd92b9500f831588db17d0ad5d4f628cfbda7b36121a0941728432088c0e2`. This is not the current deployment pin. Actual Telegram polling, production HTTPS, imported ordinary-account access, fresh-role admin denial and readiness passed. The diagnostic proxy was removed; the cutover runtime used the direct Bot API with no legacy worker or fixture entrypoint. Actual native backup and inactive restore passed; maintenance normally stopped and resumed the same original container. Archives are private plaintext.

The matching image pair passed menu, native-product and real-browser acceptance;
1,142 Rust tests passed. YouTube OAuth and PoToken issuance remain synthetic
acceptance evidence, not a claim of live account authorization. Native writes
are now possible. Do not restart stale PostgreSQL writers as a rollback.
