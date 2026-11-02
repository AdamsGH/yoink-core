# YouTube service fixtures

The repository retains a standalone topology for isolated adapter and worker checks. It is not the default combined production container.

## Commands

```sh
just --justfile backend/justfile test-youtube-auth-package
just --justfile backend/justfile test-youtube-auth-gateway-node22
just --justfile backend/justfile test-youtube-auth-topology
just --justfile backend/justfile test-youtube-auth-worker
```

The topology runner expects its reviewed images. It does not build or pull them implicitly.

[Topology configuration](../../../backend/docker-compose.youtube-auth.yaml) and its [fixture override](../../../backend/docker-compose.youtube-auth-fixture.yaml) define the isolated services.

## Boundaries

Each run uses a unique project and socket volume. The deterministic override substitutes synthetic upstreams and removes public provider egress.

The worker mounts the socket read-only. Personal credentials do not enter arbitrary command arguments, logs, or ambient environment settings.

The gateway uses bounded admission with no queue. Uncertain upstream dispatch cannot release authority as if no request occurred.

## Assertions

Fixtures cover fixed-origin adapter calls, request and body limits, cancellation, deadlines, scratch limits, file cleanup, and process reaping.

Worker checks use the actual confined launcher. Local FFmpeg output proves only that tested synthetic media path.

Bundled service checks cover the current supervisor, health, private socket, and component lifecycle separately from the standalone topology.

## Cleanup and evidence

Cleanup allows dependency shutdown and checks for residual owned resources. Failure reports remain private and bounded.

Synthetic OAuth responses, bgutil ping, and canned provider payloads do not establish live account authorization or PoToken issuance.

These checks do not establish destructive OOM/PID/tmpfs exhaustion behavior. Report only the scenarios actually executed.
