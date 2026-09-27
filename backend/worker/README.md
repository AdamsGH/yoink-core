# Python media executor

The worker consumes one version 1 JSON request from stdin and emits newline-delimited version 1 events on stdout. Run one process in a private, empty per-attempt directory. Its only output artifact is media.mp4 or media.webm. Classified failures produce one terminal failure event and exit status 0; invalid envelopes or invalid worker configuration produce no events and a nonzero status. Stderr is suppressed to protect source URLs and library diagnostics.

Only progressive HTTP(S) video and best downloads are supported. Audio-only and content extraction requests are classified as unsupported. Adaptive HLS/DASH, audio extraction, and parity with other media workflows are out of scope. The worker uses yt-dlp's Python networking only, disables plugins/configuration/cookies/proxies/external downloaders, and does not use Telegram, database, or authorization services.

The worker resolves and checks every destination address on connect, including redirects. Private, loopback, link-local, multicast, reserved, and IPv4-mapped private destinations are rejected unless the exact origin hostname and port is explicitly present in YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS. The allowlist is intended for controlled private fixtures only. It does not authorize arbitrary redirect destinations. This guard is for progressive HTTP only and does not establish safety for future adaptive-streaming or audio workflows.

Validated limits and configuration:

- YOINK_WORKER_MAX_BYTES limits the downloaded artifact to 1 through 52428800 bytes, default 52428800. Downloads exceeding the limit are classified as resource_limit.
- YOINK_WORKER_SOCKET_TIMEOUT_SECS is the yt-dlp socket timeout and the ffprobe wall-clock validation deadline, from 1 through 300 seconds, default 30. ffprobe stdout is streamed in 4 KiB reads and capped at 65536 bytes; oversized output or deadline expiry terminates and reaps ffprobe. Its stderr is discarded.
- YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS is a JSON array of explicit HTTP(S) origins, default []. Origins contain only scheme, host, and optional port, for example `["http://127.0.0.1:8123"]`.
- Transient HTTP 408, 429, 500, 502, 503, and 504 responses are classified respectively as source_unavailable or rate_limited for 429. Authorization, not-found, and other HTTP failures are not classified as transient.

Run the stdlib unittest suite in the experimental image. The recipe mounts this directory read-only, disables container networking, and uses loopback-only fixtures with the image's pinned yt-dlp and ffmpeg/ffprobe:

```sh
cd backend
just build-media
just test-worker
```

The worker entrypoint is /opt/yoink-worker/main.py. The container runner must provide an isolated writable working directory per attempt and clear proxy environment variables.
