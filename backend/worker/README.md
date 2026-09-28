# Python media worker

The worker consumes one version 2 JSON request from stdin and emits version 2 NDJSON events on stdout. Each process handles one attempt in an empty private workspace. A successful terminal event contains an ordered, bounded artifact list. Classified download failures emit one failure event and exit successfully. Invalid envelopes and invalid configuration emit no events and exit unsuccessfully. Library diagnostics do not belong on stdout, and the production entrypoint suppresses stderr logging.

`main.py` validates the envelope and configures networking. `media_engine.py` uses pinned yt-dlp and gallery-dl libraries for video, audio, clips, selected playlist entries, image batches and optional gallery ZIP output. Arbitrary downloader arguments, ambient configuration, plugins, browser cookies and downloaded executable components are disabled. Content extraction is not implemented. Python-native adaptive downloads may run, but external FFmpeg networking is prohibited. End-to-end adaptive-streaming coverage remains required before claiming compatibility.

FFmpeg and ffprobe run through immutable image-owned wrappers under `worker/bin/` that restrict protocols to `file,pipe` and allow only selected local demuxers. No executable files are created in the noexec workspace. Their execution and output are bounded. The engine probes output before producing artifact metadata. The Rust executor independently checks file types, sizes, aggregate limits and open-file identity before handing artifacts to delivery.

## Isolation and networking

The experimental runtime launches `/usr/local/bin/yoink-worker-launcher`, which requires Linux Landlock ABI 6 or newer. It grants write access only to the attempt workspace, read access to required runtime paths, and restricts signals and abstract Unix sockets. Unsupported kernels fail closed. This is filesystem and IPC confinement, not a kernel network sandbox.

The Python network guard validates source URLs and every urllib/requests redirect, rejecting private, loopback, link-local, multicast, reserved and mapped-private addresses unless the exact source hostname and port is explicitly allowed. The source allowlist does not authorize unrelated redirects.

A supplied HTTP or HTTPS proxy goes through a local CONNECT relay. The relay resolves each destination itself, checks every returned address, then sends a numeric-IP CONNECT request to the upstream proxy. A distinct proxy allowlist permits operator-approved private proxies without granting source access to them. Direct requests to the local relay and bypassing it through the upstream proxy are rejected. These checks protect the configured download paths, not arbitrary Python programs.

Network credentials enter only through the request's redacted stdin section. They do not belong in arguments, environment variables, logs or persisted job JSON. Netscape cookies use a temporary mode-0600 file inside the workspace and are removed on completion or failure. Gallery cookie writeback is disabled. Runtime credential selection, OAuth refresh and cache authorization belong to the Rust service and are not implemented by this worker.

## Worker environment

The Rust runtime may impose tighter limits than the worker's standalone bounds.

- `YOINK_WORKER_MAX_BYTES` limits each artifact to 1 through 2147483648 bytes. The standalone default is 52428800.
- `YOINK_WORKER_TOTAL_MAX_BYTES` limits aggregate artifact bytes to 1 through 8589934592. The standalone default is 52428800.
- `YOINK_WORKER_MAX_ITEMS` limits artifacts to 1 through 100. The standalone default is 20.
- `YOINK_WORKER_SOCKET_TIMEOUT_SECS` limits network waits to 1 through 300 seconds. The standalone default is 30.
- `YOINK_WORKER_PROCESSING_TIMEOUT_SECS` limits each local media-processing command to 1 through 86400 seconds. The standalone default is 300. The executor also imposes a whole-attempt deadline.
- `YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS` and `YOINK_WORKER_ALLOWED_PROXY_ORIGINS` are separate JSON arrays, both empty by default. Origins contain only scheme, hostname and optional port, with no credentials, path, query or fragment.

Budget failures are terminal `resource_limit` events. Expected HTTP failures map to fixed categories, including `rate_limited`, `invalid_media` and `source_unavailable`. Exceptions never expose source URLs or credentials through protocol errors.

## Verification

Python runs only in the experimental container. Tests mount the worker directory read-only and use loopback fixtures with container networking disabled.

```sh
cd backend
just test-worker
just test-worker test_gallery_integration.py
just test-worker test_proxy_tunnel.py
```

After `just build-media`, `just test-worker-launcher` checks the exact default launcher and isolated Python imports inside the read-only, noexec, capability-dropped deployment profile. `just test-media` exercises real processing and native Bot API uploads under that same profile.

The gallery tests exercise the pinned gallery-dl download/write path, per-file and aggregate limits, partial-source failures, ZIP ordering and protocol-only subprocess stdout. `just test-media gallery` adds the unchanged Imgbox extractor, synthetic multi-page HTTPS source, native photo/document albums, mixed photo/ZIP sends, small-image cache recovery and durable partial delivery. The fixture checks both rejected untrusted TLS and successful trusted TLS. Its DNS and certificate overrides exist only in disposable test containers. Worker tests do not replace the Rust executor, confinement, native database or Bot API integration gates.
