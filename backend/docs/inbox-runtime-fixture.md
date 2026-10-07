# Native Inbox actual-process fixture

This fixture creates only a randomly named Docker volume and tracked containers,
uses the fixture-only native SQL controller, and deletes owned resources in
`finally`. It accepts no deployment database path, deployment token, user-content
private-origin exemption, or PostgreSQL source URL. No Cargo/image build or pull,
production data access, or host Python execution occurs here.

## Coordinator execution with supplied integrated images

The coordinator verified the integrated native runtime, workspace Clippy/tests
and the hardened launcher. All nine process scenarios passed with matching
immutable images. Independent review then identified four false-positive gaps
in storage, import and SPA checks. After repairs, those three scenarios passed
again. Live confined-worker shutdown remains an open acceptance gate.

The supplied images have separate roles: production server for migration,
fixture image for actual runtime/SQL control, operations image for backup/restore.
Run the seven independent native scenarios with those exact immutable IDs:

```sh
node --test backend/scripts/inbox-fixture/*.test.mjs
node backend/scripts/test-inbox-runtime.mjs \
  --server-image sha256:b85f5d4eb8fe8c357faaba8a9765792ae5d4821bfc563ec3ab0da8b9d142a188 \
  --fixture-image sha256:fd7c73e1fea2a64bd4f41f500d747920aa627bdf1ec1b86faafda587c54cc863 \
  --operator-image sha256:cf20475cbd48539392fd43a2bbf7292278f1a78dd83dceb66edc32de071ff5b8 \
  --scenarios routing,httpContracts,telegramContracts,groupContracts,shutdownContracts,deadlineContracts,storageContracts
```

This explicitly selected run is **not full acceptance**: final JSON includes
`selected`, `notRunScenarios` and additional remaining gates. With no `--scenarios`
argument all nine scenarios run; missing importer/SPA dependencies fail explicitly
without silently skipping independent native scenarios.

For all scenarios, also supply coordinator-built immutable importer and frontend
images, using local image IDs or repository digests:

```sh
node backend/scripts/test-inbox-runtime.mjs \
  --server-image "$INBOX_SERVER_IMAGE" \
  --fixture-image "$INBOX_FIXTURE_IMAGE" \
  --operator-image "$INBOX_OPERATIONS_IMAGE" \
  --importer-image "$INBOX_IMPORTER_IMAGE" \
  --frontend-image "$INBOX_FRONTEND_IMAGE" \
  --chromium /usr/bin/chromium
```

Prerequisites: Linux Docker host networking, Landlock confinement, preloaded
images and Chromium supported by `media-fixture/browser-client.mjs`. The server
and fixture images contain the fixed launcher, container Python, worker source,
`sh`/`sha256sum`, and `yoink-server`; only the fixture image needs
`yoink-fixture-server`. `backend/Dockerfile`'s operations target contains only
`yoink-ops`; its importer target contains only `yoink-import`. The fixture does
not incorrectly require a combined operator image. The frontend image must serve
the **existing built SPA** on port 80 with its normal Nginx entrypoint.

Native Inbox has no separate worker-image switch: Python runs in the server
container through fixed launcher paths. Optional `--worker-image` defaults to
`--server-image`; SHA-256 fingerprints verify that server, fixture and supplied
worker contain matching worker-main/launcher bytes. Coordinator also owns pinned
parser dependency verification. Host fixture binary execution is unsupported
because the fixed Python path would otherwise execute host Python.

The read-only runtime mounts writable `/tmp` and uses `/state/inbox` as its
working directory; this avoids hiding the image's `/tmp/yoink-work` working
directory under an empty tmpfs. SIGINT/SIGTERM stop further admission and enter
bounded failure/cleanup paths. Reports include bounded, redacted native logs and
job/notification snapshots.

## Nine actual-process scenarios

1. **routing:** disabled Inbox/no bearer return 404; missing session returns 401;
   a held established Bot API menu request exposes diagnostics before preparation,
   followed by late real readiness.
2. **httpContracts:** real signed Telegram WebApp login; current bans/features
   with unchanged tokens; user isolation; save/list/detail; invalid inputs and
   cursor pagination; local folder membership; real stars sync and local-row-ID
   writes; remote list operation-key replay/conflict; lost create receipt,
   candidates, no resend across restart and explicit candidate adoption.
3. **telegramContracts:** actual `/save` replay and distinct-duplicate creator
   semantics; GitHub README Content extraction and real Insight adapter completion
   via the trusted fake gateway; edits target the proven bot reply, not the user
   message. A proxy forwards requests to the established Bot API fake before
   dropping one real send/edit receipt. Unknown effects do not retry. A confirmed
   actual initial receipt recovers its binding after a simulated local crash gap;
   no receipt is manufactured.
4. **groupContracts:** established membership updates plus actual group/thread
   HTTP controls; disabled-topic denial, enabled-topic receipt, admitted read
   shutdown, restart after topic revocation denying completion, member-left denial.
5. **shutdownContracts:** admitted real GitHub read SIGTERM restores attempts and
   releases leases; restart completes. A real confined worker denies an untrusted
   loopback URL without reaching the fake service (terminal denial, one attempt).
   Bounded stars-read failures consume two attempts; later sync completes.
6. **deadlineContracts:** with job=6s and GitHub-request=4s, fake stars page one
   completes after 3s and page two remains held. The real outer job deadline must
   cancel page two **before its own 4s adapter deadline**, on both attempts. The
   fixture checks actual socket-close timing, terminal attempts/cleared lease,
   no further dispatch and subsequent successful work. This is distinct from
   both single-adapter timeout and owning shutdown; the fake timing helper test
   does not itself prove native deadline behavior.
7. **storageContracts:** real idempotent migration rerun; valid empty SQLite
   database rejected by both SQL and startup schema checks; invalid archive
   restore with no publication. The fixture creates an actual admitted job,
   stops it, verifies its restored `ready` state, and compares nonempty durable
   rows after backup/restore. The original job resumes afterward. Rejection
   requires authoritative container state, native exit 1 and its exact diagnostic.
8. **importContracts:** actual importer malformed-source rejection with no
   publication. Missing `--importer-image` is a precise reported dependency and
   does not prevent backup/restore acceptance from running.
9. **spaContracts:** existing CDP browser and private disposable profile open the
   real built SPA through a same-origin forwarding proxy to the native server:
   feed, detail dialog, stars/folders and session reload. It seeds the actual
   session token once in an inert setup document, not on each navigation. Reload
   must retain the token and issue fresh authenticated native requests. Its
   prerequisites use actual native routes. No Inbox API response or page is
   reimplemented. Missing frontend image fails this scenario.

All SQL crash-gap changes affect the owned temporary database only. GitHub and
completion origins are operator-trusted config; executor private-origin/proxy
allow-lists remain empty. The configured `INBOX_INTERVAL_MS=100` now applies to
notification polling as well as jobs in the integrated server.

Original-process graceful exit must be zero, not OOM, and carry fixture_main's
`/proc` child-reaping proof before removal. Persistent Inbox workspace emptiness
is separately checked by an offline helper. The seeded fixture_main's workspace
signal snapshot is **media-only** (`config.media`); its `workspaces=0` line alone
is not a live Inbox-worker workspace proof.

## Remaining full acceptance gates

* The separate PostgreSQL suite passed against the integrated code: 54 real
  Alembic revisions, independent comparison of 41 tables and 504 cells, TLS
  failures, native reopen and backup/restore safety. `importContracts` checks
  malformed-source rejection only and does not replace that suite. The existing
  `backend/scripts/import-fixture/test-import.mjs` owns full import acceptance.
* Admitted **live confined-worker child** shutdown/restart needs a deterministic
  public destination held while the anonymous worker runs, without widening
  private-origin permissions. The implemented trusted GitHub cancellation gate
  does not claim to be that gate.

## Helper checks

`just test-inbox-fixtures` from `backend/`: 33 passed, zero failed.
Checks include immutable role-specific images, scenario selection/invalid inputs,
optional importer handling, deterministic sessions, bounded polling/reports and
streamed responses, LIFO cleanup despite failures, fake wire formats, lost
send/edit receipts, paginated held-read timing and socket cleanup. Native
rejection tests reject Docker wrapper errors, startup failures, OOM and wrong
exit statuses as acceptance evidence.

Helper tests alone do not prove native/container/browser behavior. Process
verification and remaining acceptance requirements are described above. All
fixture state is disposable; deployment data and credentials are not inputs.
