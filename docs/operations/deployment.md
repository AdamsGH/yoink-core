# Deployment and rollback

The supplied Compose deployment runs the native Rust backend. PostgreSQL, Redis, and ARQ remain outside default startup. They do not receive current native writes.

## Services

| Service | Role | Default host exposure |
| --- | --- | --- |
| `yoink` | Bot, HTTP API, and native products | 8003. |
| `yoink-frontend` | React Mini App | 3010. |
| `yoink-nginx` | HTTP edge | 8081. |
| `tg-bot-api` | Local TDLight Bot API | Loopback 8082. |
| `yoink-proxy` | HTTP facade over the host proxy | Internal only. |
| `native-youtube-auth-gateway` | YouTube providers and Unix adapter | Unix socket only. |

The [installation guide](installation.md) covers fresh hosts and the separate Bot API image. The [Compose manifest](../../docker-compose.yaml) defines ports, resource limits, mounts, and health checks.

## State and isolation

Native state lives in `data/native`, owned by UID/GID 10001. The backend has a read-only root and separate private product workspaces.

Bot API files mount read-only at `/telegram-files`. The runtime needs read/traverse ACLs for UID 10001, including defaults for new files.

The combined YouTube service exposes only its private Unix socket to consumers. Its supervisor checks the providers and drains the gateway during shutdown. See [YouTube service operation](youtube-services.md).

## Upgrade the backend

1. Review schema and persisted-request compatibility.
2. Create a verified native backup.
3. Build or download the matching release.
4. Register the candidate with the [release manager](releases.md).
5. Run `just --justfile backend/justfile release-deploy`.
6. Check readiness, authentication, bot behavior, and the changed product.
7. Confirm the release only after human acceptance.

The release manager does not run database migrations. A schema upgrade requires a stopped writer, an explicit migration with the selected compatible image, and a recovery plan. Do not start an older incompatible binary after that migration.

Backend deployment replaces only `yoink`. Frontend and auxiliary changes need separate image updates and container recreation.

Deploy matching frontend and backend contracts together. An old SPA can continue running in an open browser tab after the frontend container changes.

## Rollback

`release-rollback` restores the retained runtime image. It does not restore database contents, downgrade schemas, or roll back frontend and YouTube services.

A previous binary may reject a newer schema or persisted request type. Use a compatible binary or an explicitly selected pre-upgrade backup. Never rewrite journals to force compatibility.

Do not restart the legacy PostgreSQL application after native writes. Its database is stale. Returning to it requires a verified reverse migration or an explicit data-loss decision.

## Backup and restore

`just backup run` stops the native writer, creates a verified archive, and resumes the same runtime. Check readiness afterward.

`just backup restore ARCHIVE` writes an inactive database under `data/native-restore`. It does not activate that database.

Archives contain plaintext credentials and private content. Store and transport them privately. Native backups have no automatic S3 schedule in the supplied deployment.

See [backup and restore](backup-restore.md) for archive validation, crash recovery, and external-effect reconciliation.

## Legacy migration

The PostgreSQL importer is an offline migration tool, not runtime replication. It publishes a new verified native file without replacing a live database.

A migration requires stopped source writers, equivalence checks, a restore rehearsal, and exclusive bot polling. See [the import procedure](import.md).
