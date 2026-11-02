# Yoink

Yoink is a self-hosted Telegram bot and Mini App for media downloads, music links, group statistics, AI summaries, and saved content.

The current runtime is Rust with an embedded Turso database. A confined Python worker handles media and content extraction. The React frontend includes pages from the plugin repositories. Python bot services and PostgreSQL remain legacy migration and recovery tools, not the default deployment.

## Features

| Product | Use |
| --- | --- |
| [Downloads](docs/features/downloads/README.md) | Download media, select formats, manage personal cookies, and review download history. |
| [Music](docs/features/music.md) | Resolve music links across providers and optionally send audio. |
| [Stats](docs/features/stats.md) | Read authorized group statistics and search indexed messages. |
| [Insight](docs/features/insight/README.md) | Summarize content, manage prompts, and configure AI providers. |
| [Inbox](docs/features/inbox/README.md) | Save content, classify items, and organize GitHub Stars. |

Access depends on current roles, feature grants, and group policies. A supported provider can still reject a request because of authentication, region, or upstream changes.

## Install

Use the [installation guide](docs/operations/installation.md) for source builds or published Linux amd64 images. It covers credentials, storage permissions, the separate Bot API dependency, and first startup.

Clone the repository with its frontend submodules:

```sh
git clone --recurse-submodules https://github.com/AdamsGH/yoink-core.git
```

Do not start a second poller with a bot token that another deployment uses.

## Documentation

- [Documentation index](docs/README.md)
- [Architecture](docs/architecture/overview.md)
- [Configuration](docs/operations/configuration.md)
- [Deployment and rollback](docs/operations/deployment.md)
- [Backup and restore](docs/operations/backup-restore.md)
- [Development and checks](docs/development/verification.md)
- [HTTP API](docs/architecture/api.md)
- [Releases and image publication](docs/operations/publication.md)

The [GitHub Wiki](https://github.com/AdamsGH/yoink-core/wiki) uses an export of the public documentation. The repository files are the source of truth.
