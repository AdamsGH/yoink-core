# Inbox actual-process fixture

The fixture uses disposable native state, synthetic credentials, owned containers, and the fixture-only SQL controller. It does not accept a production database path or discover deployment credentials.

## Run

Provide matching verified server, fixture, operations, importer, and frontend image references before running:

```sh
node --test backend/scripts/inbox-fixture/*.test.mjs
node backend/scripts/test-inbox-runtime.mjs \
  --server-image "$INBOX_SERVER_IMAGE" \
  --fixture-image "$INBOX_FIXTURE_IMAGE" \
  --operator-image "$INBOX_OPERATIONS_IMAGE" \
  --importer-image "$INBOX_IMPORTER_IMAGE" \
  --frontend-image "$INBOX_FRONTEND_IMAGE" \
  --chromium /usr/bin/chromium
```

The driver requires immutable matching images. It does not build or pull them. Server migration, fixture control, operations, importer, and SPA images have separate roles.

The host needs Linux Docker networking, Landlock support, and a supported Chromium executable.

## Scenario selection

Omitting `--scenarios` runs the full set. A selected subset reports its omissions and is not full acceptance.

Routing, HTTP, Telegram, group, shutdown, deadline, storage, import, and SPA scenarios have distinct assertions. Missing required dependencies fail rather than silently skipping a selected gate.

## State and external effects

The fixture exercises ingestion, classification, local folders, explicit GitHub mutations, stable keys, receipt recovery, and unknown-effect refusal.

Synthetic GitHub responses do not perform real account writes. Public content and real credentials are not fixture inputs.

Storage checks include crash/restart, schema verification, archive/restore, and immutable receipt behavior. The separate [import fixture](../../../backend/scripts/import-fixture/README.md) owns full PostgreSQL equivalence checks.

## Shutdown evidence

Graceful shutdown must come from the original process. Forced container removal, OOM, and a replacement process cannot establish successful drain.

The live-worker gate correlates blocked DNS with the actual child socket. It checks process identity, confinement, private workspace ownership, and signal-boundary live-child evidence.

The original process must reap children and release eligible lease state. Restart then verifies recovery without duplicate item, job, or Telegram effects.

A media-only workspace summary does not prove the Inbox workspace is empty. Inbox cleanup requires its own observation.

## Reports and cleanup

The driver removes only owned volumes, networks, containers, and temporary files in cleanup. Cleanup failure remains an error.

Reports distinguish passed, failed, and unrun gates. Node orchestration tests alone do not prove native process, container, or browser behavior.
