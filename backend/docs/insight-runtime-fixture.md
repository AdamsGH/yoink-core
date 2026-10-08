# Native Insight actual-process fixture

Entrypoint: `backend/scripts/test-insight-runtime.mjs`. The coordinator builds matching immutable images and runs actual-process acceptance. Node helper checks alone are not native acceptance evidence.

## Invocation

Use immutable matching images built after current main's server/HTTP and Telegram integration, plus the separately built SPA:

```sh
node backend/scripts/test-insight-runtime.mjs \
  --server-image sha256:<server-id> \
  --fixture-image sha256:<fixture-id> \
  --worker-image sha256:<matching-worker-id> \
  --frontend-image sha256:<SPA-id> \
  --chromium /usr/bin/chromium
```

`--scenarios` selects a comma-separated subset of the gates below. Omitting the SPA image while selecting `spa` fails explicitly. Mutable image references, unknown/duplicate options, and unknown/duplicate scenarios are rejected. A failed gate aborts the sequence; the report distinguishes passed, failed, and not-run gates. Forced cleanup cannot convert a native failure into a pass. Even a successful selected run is labelled `selected-process-gates-passed`, not full acceptance: the remaining coordinator interfaces below are still required.

## Executable gates

Run `forcedReceipt` first in a fresh fixture database. It must observe a genuine cache miss followed by a hit; earlier gates can populate that identity's cache. The default order preserves this requirement.

| Gate | Required native/process observations |
|---|---|
| `routing` | Disabled standalone Insight is 404; enabled Insight remains independent of disabled Inbox. Hold actual Bot API menu preparation and require readiness 503 before releasing it; ready routes require authentication. Existing system status reports disabled Insight as planned and configured Insight as experimental, with readiness asserted separately. |
| `identity` | Real HMAC WebApp login issues JWT; owner/user authorization, invalid login and fresh ban checks. Public wildcard and limited API keys authorize internal M2M operations, not Insight users. Require key-only Insight requests to fail, JWT-plus-key to use the JWT identity, invalid JWT to stay rejected, and revoked keys to fail internal authentication. |
| `management` | Saved gateway-key probe reaches the trusted native catalogue. Credential omission preserves exact stored key; explicit empty clears. Config responses omit values. HTTP alias create/update/list/delete. |
| `policy` | Non-owner model allowlist enforcement, invalid default-model rejection, native permitted catalogue and admin denial. |
| `byok` | Native PUT and omitted-key probe use the saved credential but fail private-address safety/probing; explicit empty probe reports `api_key_required`. PUT omission preserves the exact key and clearing removes readiness. Global toggle, public summary grant/revoke, BYOK clear/delete and actual personalized menu invalidation. An explicitly imported tested row is setup for invalidation only, **not successful-probe evidence**. |
| `summary` | HTTP TLDR traverses actual fixed-launcher full-content worker, exact operator proxy and native completion. Replay **before exhaustion** is cached and avoids completion. Under 20 isolated current reservations, the same TLDR request must return **429 `quota_exceeded` without extraction or completion**. Another user cannot reuse the owner's private native cache. |
| `cacheIsolation` | Real replay is partitioned by provider credential replacement and source-token clearing. Import legacy unverifiable rows; discard only test-owned native replay state and require real extraction/completion rather than serving imported content. Preserve imported rows byte-for-byte, including IDs and expiry. |
| `failures` | Deterministic content 503 and provider 503 remain native failures with their exact public error/status contracts; quota reservations drain, and a later real request succeeds with remaining capacity. |
| `telegram` | Actual Bot API update executes native TLDR with real worker. Require native streaming request and final Unicode delivery after deliberately fragmented SSE JSON and UTF-8. |
| `cancellation` | Abort a live HTTP client during held native completion; require reservation release and upstream closure. Cancel a held native TG stream using its published UUID; require upstream/quota drain, no final result, and subsequent real work. |
| `revocation` | Fresh DB ban during held native completion rejects the unchanged session and releases quota. |
| `heldRevocation` | Revoke the shared TLDR grant or clear the provider credential during held HTTP work. Require `access_denied` or `route_changed`, respectively, no successful result, and released reservations. Restore isolated state in `finally`. |
| `telegramRevocation` | Revoke grant after a real incomplete TG streaming draft; release provider and require no final delivery and released quota. Later authorized work remains possible. |
| `oauth` | Actual native read/write device polling against owned canned operator endpoints. Read/write token separation; missing write scope cannot persist; expiry and denial; disconnect during held polling; replacement cancels old generation and a late old credential cannot overwrite the new token. Explicit persisted-token absence and a final successful polling barrier. |
| `oauthStart` | Submit malformed replies for 1025 distinct authorized fixture users against the unchanged 1024-slot bound; each failed START must leave status none. Hold START replies for final-authorization rejection and real HTTP disconnect, for both read/write. A successful replacement must survive release of the abandoned START, original-process shutdown, and persisted-token checks; the abandoned generation must never poll. No imported flow map or reduced admission bound. |
| `abortedQuota` | Observe a real reservation at a held provider, abort its HTTP client, then observe zero reservations and unchanged successful usage. Repeat with no cleanup wait before original TERM, checking zero reservations and unchanged usage through the offline DB after process exit. Restart and complete real work. No native reservation is deleted to make this gate pass. This observes cleanup/join outcomes; it does not by itself prove a particular scheduling interleaving or a detached-task bug. |
| `receiptShutdown` | Retain the real canned Bot API response after observing ledger state sending with null receipt. Original TERM must close the stalled receipt; late release cannot confirm or mutate the closed-store effect. Restart, replay the same incoming message, drain handlers, and require exactly one external attempt for that message. This is ordinary drain evidence, **not forced-TG-adapter-abort evidence**. |
| `forcedReceipt` | Prime the genuine cache without focus, hold final receipt confirmation after Bot API validation, signal promptly, require exact Telegram task abort-and-join at the unchanged 90-second deadline, release, and verify handler/concrete/database shutdown ordering and original exit 1 with ShutdownTimeout. Restart, arm the exact-token observation, require one existing-ledger replay fence and a separate live handler, then verify no duplicate send or ledger mutation. |
| `shutdown` | Hold both a live content worker and actual OAuth polling. Send TERM to original native process; require signal-boundary live-child evidence, exit 0, no OOM/error, child-reaping log, empty configured Insight workspace and zero reservations. After exit, neither late read nor write token is persisted. Restart is separate from original-process proof. |
| `heldDns` | Existing public Inbox DNS helper is confined to this fixture's internal network. Hold the exact worker hostname; read-only `/proc` shows the actual immutable worker, private workspace, unchanged UID/capabilities and an owned Docker DNS-stub socket. Original TERM must reap the child **without releasing DNS**; workspace and quota must be clean. Docker stub forwarding means this does not claim direct equality between child's socket port and helper's forwarded-query port. |
| `spa` | Execute separately built SPA against native API, with confined Chromium and real login. Settings/admin navigation; admin unchanged/replace/reset/clear/undo; GitHub token replacement, unrelated-save omission, clear/undo; real alias dialog create/edit; BYOK saved-key/replace probes, omission/replacement PUT and clear; native read/write OAuth forms, actual polling, write revoke preserving read token. Inspect credential field semantics, never values. Native API assertions alone do not count as form execution. |

## Current native interfaces and confinement

Read current main `insight_config.rs`, `insight_runtime.rs`, product HTTP/OAuth modules, SQL and integrated frontend read-only. Runtime config uses `YOINK_RUST_INSIGHT_ENABLED`, `GATEWAY_BASE`, `GATEWAY_KEY`, `MODEL`, `WORKSPACE`, and **`CONTENT_PROXY`** (all with the `YOINK_RUST_INSIGHT_` prefix). There is no invented `ALLOWED_PROXY_ORIGINS` environment interface: composition derives executor proxy origins from trusted config. Budgets are request 20s, text 5s, content/research 15s, socket 10s, OAuth request 10s and username 3s, inside native shutdown 90s. Limits are explicitly TLDR 20 / summary 50. SQL timestamps/reservations use native integer microseconds.

Public OAuth routes are `POST /github/login`, `GET /github/login/status`, `POST /github/upgrade-scope`, `GET /github/upgrade-scope/status`, `GET /github/write-token/status`, and token DELETE endpoints. No `/device/start` or separate public `/cancel` route is invented. Token DELETE disconnects/cancels; repeated start fences the old generation. Successful status is `success`; terminal provider/scope errors expose `oauth_failed`.

* Allocate a random owned **internal Docker network**, inspect `Internal=true`, and expose native HTTP through an owned host-loopback TCP relay to the exact inspected app address. Docker internal-network port publishing is not assumed. Native server is read-only, zero-capability, no-new-privileges, with bounded memory/PIDs/tmpfs. No host DNS changes, source-mounted server code, public egress, live providers or Telegram.
* The exact canned content proxy allows absolute-form `GET http://93.184.216.34/article` and CONNECT only to `93.184.216.34:80`. CONNECT terminates in a local HTTP parser, never an upstream socket; only the exact Host and article path succeed, and `/llms.txt` returns 404. Arbitrary authorities, URLs, private sources and redirects are denied. The immutable worker still performs normal source validation/extraction. Never inject an alternate content executable.
* Provider requests require exact paths, keys, model and actually extracted content. Unknown traffic fails closed. SSE writes split both JSON framing and UTF-8; only process observations count as decoding proof.
* OAuth's HTTP loopback-only rule is retained. `operator-relay.py` runs in a separately owned, restricted **sidecar with `--network container:<native-id>`**, a separate PID namespace, and the matching immutable image. It listens only on `127.0.0.1`, forwarding only the three native OAuth transport paths to the literal owned gateway service. No DNS, redirect, arbitrary upstream, logging of tokens, weaker TLS or source exception. Sidecar removal follows original native shutdown proof; it is not a native content child.
* The existing held DNS helper's container is adapted to the owned internal network rather than its default bridge. It retains a non-root UID, zero capabilities and its existing namespaced unprivileged-port sysctl; no widened capability or host resolver change. Native resolver receives only that helper upstream, via Docker's internal-network stub. Unknown queries receive NXDOMAIN, never addresses.
* Server/fixture/worker images must match fingerprints of embedded worker Python files and the fixed launcher. Helpers use `--pull never`; this runner never builds or pulls images.
* Containers, network and volume are registered before allocation. Reverse cleanup attempts every resource and aggregates primary/setup/cancellation/cleanup failures. Signals mark interruption and request native TERM. Failed SIGTERM assertions stay failures even if forced teardown succeeds.
* SQL mutates only the owned fixture database. Live mutations use the existing authenticated native fixture controller through bounded container-only `socket-client.py`; offline operations reuse `native-db.mjs`. Python transport/evidence/sidecar code executes only inside containers. No host Python is used.
* Original fixture log snapshots currently select media's workspace. They prove actual original child observation/reaping, **not Insight workspace selection**. The fixture independently requires `/state/insight` empty after native exit. Workspace paths and lifecycle remain process assertions, not source-based acceptance claims.
* Diagnostics are bounded and redact known JWT, API/provider/OAuth/controller secrets. Proxy/browser transcripts record paths, statuses, authentication booleans and omitted/replace/clear categories, not credentials.

## TLS topology and acceptance

The identity gate follows the executable legacy contract. Insight uses JWT user identity through `get_current_user` and `require_role`. `X-Api-Key` belongs to scoped internal M2M operations; `*` does not impersonate the key creator. Native mounting retains that separation. WebApp identity uses authenticated HMAC login, not a raw WebApp header.
The positive gates use `tls-topology.mjs`: internal public-address data and private control networks, exact offline DNS records, hostname-verified HTTPS, fixture-only native DER roots, and a container-generated standard Python CA bundle. The app shares the TLS fixture's owned network namespace so traffic to its public-class address remains local rather than entering host policy routing. PID/mount namespaces remain separate; the app cannot read fixture private keys. All networks and trust mounts are installed before startup. Production trust and source-address policy are unchanged. The endpoint accepts exact Host names with optional default HTTPS port 443, retaining SNI matching.

`positiveByok` requires real stored-key probes and enablement. `positiveCacheIsolation` requires successful BYOK and gateway private replay partitioning. `positiveYoutube` requires actual watch/transcript extraction and fixed-origin Gemini completion for summary/about, then authorized cached replay even after daily quota exhaustion, without repeat extraction. `positiveConfinement` requires private-source, authority and TLS rejection. Imported tested rows or fabricated cache rows do not count.

Topology allocation reads the complete host IPv4 route and Docker IPAM snapshot. With owner approval, gateway routes in policy tables do not reserve addresses. Connected routes, main/local/default tables and all Docker IPAM subnets remain reserved. This changes only address-occupancy classification, not host routing. Cross-namespace public-address traffic failed on this host, whose policy route lookup selects Mihomo table 2022. Shared fixture networking avoids that path without altering Mihomo or weakening product validation. All 25 actual-process gates, including the separately built SPA, passed together. SPA transport uses the exact inspected frontend container address, not internal-network port publishing.

The DNS wire regression runs only inside the fixture image, without external networking:

```sh
docker run --rm --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
  --mount "type=bind,source=$PWD/backend/scripts/insight-fixture,target=/checks,readonly" \
  --entrypoint /usr/local/bin/python yoink/rust-backend-fixture:experimental \
  -I -B /checks/test_fixture_dns.py
```

## Bounded local checks

```sh
node --test backend/scripts/insight-fixture/*.test.mjs
for f in backend/scripts/insight-fixture/*.mjs backend/scripts/test-insight-runtime.mjs; do node --check "$f"; done
```

Twelve bounded Node tests cover option/gate validation and executable exports, ownership and failure preservation, cancellation cleanup, redaction, fail-closed content/provider paths, fragmented canned SSE, held sockets, OAuth client/scope/path validation and held poll release, deterministic failures, malformed/held START replies across replacement, and exact retained Bot API receipts with abort closure/release. These tests launch only local Node services—not Docker, Rust, Python, or Chromium.

Sources: `helpers.mjs`, `helpers.test.mjs`, `services.mjs`, `runtime.mjs`, `scenarios.mjs`, `advanced.mjs`, `oauth.mjs`, `spa.mjs`, `socket-client.py`, `operator-relay.py`, plus the entrypoint and this document. Existing Inbox/media harnesses and native/frontend sources are read/reused without modification.
