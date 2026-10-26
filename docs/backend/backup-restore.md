# Native backup and restore

`yoink-ops` is a separate Linux operator utility for the experimental native Turso 0.7.2 backend. It does not replace PostgreSQL backups, schedule jobs, change Compose settings, or authorize production cutover. Keep the CLI/image version with its backups. Cross-version restore is not established.

## Commands and ownership

From `backend/`, with explicitly chosen paths whose parent directories already exist, belong to the operator, and have mode `0700`:

```sh
just backup /private/yoink/state.db /private/backups/snapshot.ybk
just restore /private/backups/snapshot.ybk /private/restore/restored.db
```

The source DB must already exist and have the current verified schema. Migrate an older source explicitly before backup. Migration V4 adds six indexes to avoid the [native rowid-FK defect](persistence.md#native-rowid-foreign-key-workaround). It does not repair pre-existing orphan rows. Such rows fail backup validation.

The CLI acquires exclusive process ownership of the source, so stop its owning Rust process first. It cannot attach to a live server's Store or replace that server's lock. The library API accepts a Store already owned by the caller. Its native snapshot phase holds the serialized database connection, so concurrent writes wait or hit their normal acquisition timeout. Validation and archive copying then use the independent snapshot and release the source connection. No HTTP/admin backup endpoint exists yet.

Destinations must be new. Existing files, symlinks, ownership locks, WAL, SHM, and journal sidecars reject the operation. There is no overwrite or force option. Restore leaves the source archive and any deployed database intact. Neither command reads `.env` or switches the runtime database path.

Direct CLI flags, for either operation:

```sh
target/debug/yoink-ops backup --source /private/yoink/state.db --destination /private/backups/snapshot.ybk --max-bytes 68719476736 --timeout-seconds 300
```

The payload limit defaults to 64 GiB. This is not a bound on native snapshot staging, WAL growth, temporary disk usage, or process memory inside the engine. Budget roughly two snapshot sizes for backup staging plus source/WAL space, and one payload size plus validation overhead for restore. The streaming archive layer uses a 64 KiB buffer. Dataset-scale throughput and sizing remain unmeasured.

The 300-second default deadline, SIGINT, and SIGTERM request cooperative cancellation. The current native operation must finish before cleanup, so the deadline is not a hard wall-clock limit. Chunked archive copies observe cancellation between chunks. Exit 130 means cancellation before publication, exit 2 means invalid CLI arguments, and other nonzero exits mean failure. After publication has linearized, success wins over a concurrent cancellation.

## Container CLI

`just build-ops-image` tests and builds the binary with the pinned Rust toolchain as `yoink/rust-operations:experimental`. It contains neither Python nor the server entrypoint. Match the operator UID/GID to the files on the mounted filesystem. For example, with an already-private state directory containing the stopped source DB:

```sh
docker run --rm --pull never --network none --read-only --cap-drop ALL --security-opt no-new-privileges --user "$(id -u):$(id -g)" --tmpfs /tmp:rw,nosuid,nodev,noexec,mode=1777 -e TMPDIR=/tmp -e TURSO_TMPDIR=/tmp --mount type=bind,source=/private/yoink,target=/state yoink/rust-operations:experimental backup --source /state/state.db --destination /state/snapshot.ybk
```

The source mount must be writable even for backup: native opening/checkpoint bookkeeping and the ownership sidecar require it. Only run this against the chosen native database, never the Python deployment's data directory. A read-only container root is compatible with the explicitly writable private mount.

## Format and validation

The archive is one uncompressed file with a 56-byte header: 16 bytes `YOINK-BACKUP-v1\0`, an unsigned 64-bit big-endian payload length, and the payload's 32-byte SHA-256 digest. The payload is a standalone native database. CLI success returns JSON with `operation`, payload `bytes`, and payload `sha256`. These fields describe the archived payload, not the archive header or deployment state.

SHA-256 detects accidental corruption, not forgery or provenance. Archives contain plaintext credentials and personal data. Use trusted archives only, private storage and transport, and independently managed encryption where needed. This format is not an encrypted backup or a safe upload format for untrusted users. Archive inputs must be owner-private, owned regular files with one hard link. Symlinks, FIFOs, public files, truncation, appended data, invalid headers, wrong digests, and oversized payloads are rejected before opening a restored database.

Backup uses native `VACUUM INTO` under an owned staging directory. Turso 0.7.2 requires a literal filename and mishandles escaped apostrophes. The implementation supplies only `/proc/self/fd/<held-directory-fd>/snapshot.db`, not user-controlled SQL. Destination paths containing quotes and spaces are supported.

Both paths validate the staged DB's migration ledger and schema fingerprints, require an affirmative `integrity_check`, and check declared foreign keys with explicit anti-joins. They do not assume an unsupported `foreign_key_check` pragma returning no rows means success. They require a successful `wal_checkpoint(TRUNCATE)`, close the staged Store, and confirm no nonempty WAL remains. Only then is a mode-`0600` file synced and atomically published with no-replace rename, followed by parent-directory sync. The importer and operations utility share this publication boundary.

## Failure and crash recovery

Ordinary failure or cancellation cleans only the operation's owned staging and reservation. Existing source/destination files are preserved. A successful publication retains a private `<destination>.yoink-lock` sidecar. Restore consumers can acquire that unlocked sidecar normally.

SIGKILL before publication can leave an untrusted `.yoink-import-*` staging directory and a reservation sidecar. The prefix is shared with the importer. Do not promote files from that directory or automatically remove a lock based on its age. Retry with a new destination name. Reclaim abandoned staging only after proving the owning process is gone and checking exact ownership and paths. This utility intentionally has no stale-lock deletion command.

SIGKILL after successful publication leaves the complete artifact. If directory sync fails after rename, the command explicitly reports uncertain durability and preserves the published file. Inspect and validate it rather than deleting it or retrying over it. A valid-looking filename alone is not proof of a successful backup.

For a restore drill:

1. Keep the original database and archive untouched. Record the archive's origin, time, operator binary/image version, and reported digest outside the database.
2. Restore into a new private directory/path without starting dispatchers or Telegram polling.
3. Check the restored schema and relevant application records. Rebuild/open the derived [Stats index](stats-search.md) against the restored file. The index is not part of the archive, and the changed file identity forces a rebuild even if an old index is reused.
4. Reconcile external effects before any later activation. Restoring a historical snapshot can resurrect queued work that Telegram already delivered after that snapshot. Current lease fencing and `delivery_unknown` handling cannot reconstruct external effects absent from the backup. Do not run old and restored copies against one bot.
5. Only a separately approved cutover may change the deployment path and start one owner. Do not rename over a live database, copy its main/WAL files as a backup, or discard the original as part of a drill.

## Verification and limits

`just verify-ops` runs workspace checks, builds/tests the pinned-compiler operator image, and exercises actual Alembic import → native backup → restore with the host and container CLIs. All 41 application tables are compared independently against PostgreSQL, including wide IDs, JSON, timestamps, credentials represented by synthetic markers, and allocator high-water state. Fixture diagnostics are checked for marker leakage. Separate storage tests verify the V3→V4 upgrade and all six affected FK restrictions/cascades.

Library tests cover concurrent writes, source process ownership, collisions/permissions, corruption/limits, cancellation and dropped callers, SIGKILL before/after publication, and reconstruction of a reused derived index. The ignored unit-test entrypoint is invoked by the passing crash-parent test. Real 64 KiB container tmpfs tests exhaust space during native backup and archive restore, then verify no publication, no owned staging residue, and unchanged source content.

These checks do not simulate power loss, torn writes, failing hardware/fsync, cross-version engines, large production datasets, or every filesystem. Local Linux filesystems supporting locks, `/proc/self/fd`, and atomic no-replace rename are required. Product parity, production sizing, retention/encryption automation, and deployment cutover remain separate work. No production data or service is switched by these checks.
