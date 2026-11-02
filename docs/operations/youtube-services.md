# YouTube services

The default deployment uses a combined container for youtubei, bgutil, and the native Unix authentication gateway.

This replaces the old standalone sidecar deployment for normal operation. The separate topology remains an isolated verification fixture.

## Build and configuration

`just build gateway` builds the service image selected by `yoink_youtube_image`. Input changes require a new immutable SemVer tag.

[Dockerfile.youtube-auth-gateway](../../backend/Dockerfile.youtube-auth-gateway) and the youtubei lockfile define image and dependency inputs.

The native adapter uses `YOINK_RUST_YOUTUBE_AUTH_ADAPTER_ENABLED`. Standalone defaults disable it. Compose enables it and mounts the private socket volume.

## Credentials and lifecycle

Rust owns personal OAuth credentials, refresh, polling, expiry cleanup, and durable result retention. The sidecar does not duplicate that ownership.

Credential references remain versioned. Replacing or deleting a credential invalidates stale admitted work.

The poll budget includes database waits, provider I/O, and persistence. Shutdown must exceed that budget by more than five seconds.

Result retention gives staged replies a bounded retry window. Retries do not renew that window. Accepted tasks drain before database close.

See [cookie and OAuth contracts](../features/downloads/cookies.md) for configuration and exact authorization behavior.

## Container isolation

The container runs as UID/GID 10001 with a read-only root and bounded writable scratch storage.

Only the private Unix socket reaches consumers. Helper HTTP ports are not published.

The supervisor treats a component exit as a container failure. It drains the gateway before stopping providers and kills remaining process groups at the shutdown deadline.

Upstream stderr is suppressed because it can contain credentials. Supervisor diagnostics report component state without request data.

## Admission and recovery

Provider and gateway slots have bounded concurrency and no unbounded queue. Local validation failure does not acquire external execution authority.

An uncertain PoToken response can retain its admission slot because cancellation does not prove upstream completion. Recovery requires the documented provider/gateway restart boundary.

Health probes check the components without consuming download admission. Busy admission alone is not failure. Exhausted uncertain slots can fail readiness.

## Verification limits

Bundled-image tests verify component startup, socket permissions, health, and shutdown. Isolated topology tests exercise synthetic OAuth and media responses.

A successful ping or synthetic device flow does not establish live Google authorization or real PoToken issuance. Live account operations require separate authorization.

See [YouTube fixtures](../development/fixtures/youtube.md) for developer checks.
