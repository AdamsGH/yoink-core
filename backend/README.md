# Yoink backend

The backend runs the Telegram bot and HTTP API. It uses native Turso storage and supervises Download, Music, Stats, Insight, and Inbox services.

`yoink-server` composes the application crates. It supervises confined Python workers for media and content extraction. The runtime image does not contain the test controller.

## Documentation

- [Architecture](../docs/architecture/overview.md)
- [Configuration](../docs/operations/configuration.md)
- [Deployment](../docs/operations/deployment.md)
- [Local releases](../docs/operations/releases.md)
- [Publication](../docs/operations/publication.md)
- [Development checks](../docs/development/verification.md)
- [Persistence and recovery](../docs/architecture/persistence.md)
- [Worker protocol](../docs/architecture/worker-protocol.md)

## Standalone development

```sh
just --justfile backend/justfile verify
just --justfile backend/justfile run
```

Standalone defaults bind HTTP to `127.0.0.1:8088`. Database access, authentication, Telegram, and products stay disabled until explicitly configured. The binary does not load `.env`.

```sh
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/api/v1/system
```

Compose supplies explicit settings. Schema migration remains an explicit offline command before server startup. Use the [installation guide](../docs/operations/installation.md) for a deployed instance.
