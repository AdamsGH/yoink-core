# Documentation

Yoink runs a Rust bot and HTTP API with a React Mini App. Native Turso stores application state. The server supervises confined workers for media and content processing.

## Use Yoink

- [Downloads](features/downloads/README.md): commands, settings, cookies, history, and content rules.
- [Music](features/music.md): provider links, cards, and optional audio.
- [Stats](features/stats.md): group analytics, commands, and access.
- [Insight](features/insight/README.md): summaries, prompts, and AI configuration.
- [Inbox](features/inbox/README.md): saved items, classification, and GitHub Stars.

## Operate an instance

- [Installation](operations/installation.md)
- [Configuration](operations/configuration.md)
- [Deployment](operations/deployment.md)
- [Local releases](operations/releases.md)
- [Image publication](operations/publication.md)
- [Backup and restore](operations/backup-restore.md)
- [Legacy PostgreSQL import](operations/import.md)
- [YouTube services](operations/youtube-services.md)
- [Optional user-mode Telegram session](operations/user-session.md)

## Understand the system

- [Runtime architecture](architecture/overview.md)
- [Authorization](architecture/access-control.md)
- [HTTP API](architecture/api.md)
- [Persistence and recovery](architecture/persistence.md)
- [Worker protocol](architecture/worker-protocol.md)
- [Product lifecycle and configuration](architecture/products.md)
- [Frontend architecture](architecture/frontend.md)
- [Native database decision](architecture/decisions/0003-native-turso-runtime.md)

## Develop and verify

- [Development checks](development/verification.md)
- [Local GitHub CI runner](development/local-ci.md)
- [Media fixtures](development/media-fixtures.md)
- [Insight fixtures](development/fixtures/insight.md)
- [Inbox fixtures](development/fixtures/inbox.md)
- [Documentation and Wiki export](development/documentation.md)

Commands use the repository root unless a page states another working directory. Standalone server defaults differ from the supplied Compose deployment.
