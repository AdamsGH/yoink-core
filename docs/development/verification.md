# Development and verification

Use the pinned [Rust toolchain](../../backend/rust-toolchain.toml), Node.js 22 or newer, `just`, and Docker. Python tooling runs inside containers.

The default native checks do not require a production database or bot token.

## Main gates

| Command | Purpose |
| --- | --- |
| `just verify` | Secret-history scanning, native checks, deployment/publication contracts, and frontend verification. |
| `just check-secrets` | Full Git-history scans for core and every submodule with the shared secret policy. |
| `just test-secrets` | Real-scanner regressions for narrow exceptions, inherited rules, and history coverage. |
| `just verify-fe` | Dependency audit, translations, frontend regressions, and TypeScript checks. |
| `just --justfile backend/justfile verify` | Backend formatting, lint, tests, smoke checks, schema/script checks, and documentation. |
| `just check-docs` | Repository documentation and Wiki navigation checks. |
| `just test-docs` | Documentation tooling regressions. |
| `just ci-local` | Actual workflow browser/verify jobs in an isolated local runner. |
| `just verify-py` | Explicit legacy container-only Python checks. |

`just test` and `just lint` select native Rust checks. They are not aliases for legacy pytest or Ruff.

After committing changes, run `just check-secrets` before pushing, including documentation-only updates. History scanning does not inspect uncommitted text. Link and prose checks cannot detect secret-scanner regressions. Exceptions in `.gitleaks.toml` must retain default detection rules and pass negative controls with synthetic credentials.

See [local CI](local-ci.md) for its resource and isolation requirements.

## Focused checks

Use backend recipes for scoped work:

```sh
just --justfile backend/justfile test-crate yoink-downloads
just --justfile backend/justfile lint-crate yoink-downloads
just --justfile backend/justfile test-db
just --justfile backend/justfile check-docs
```

Recipes perform the configured resource preflight. Reuse one target directory per worktree. Separate feature graphs can duplicate build artifacts.

Browser fixtures need Chromium or Chrome 151 or newer, working browser sandboxing, and OpenSSL. `YOINK_FIXTURE_CHROMIUM` can select an absolute browser executable.

## Native database lifecycle

Provision a private parent directory before opening a database. Explicit `yoink-server migrate` creates or upgrades it.

Ordinary server startup and `check-schema` require an existing current schema. They do not migrate it.

Database tests create private temporary files, use the real binary where required, and remove their fixtures. They do not read the deployment's `.env`.

## Integration fixtures

- [Media](media-fixtures.md): actual worker execution, delivery, cache, retry, cancellation, and policy.
- [Insight](fixtures/insight.md): content/providers, streaming, TLS, OAuth, and receipt recovery.
- [Inbox](fixtures/inbox.md): ingestion, GitHub state, notifications, and shutdown.
- [Import](../operations/import.md): disposable PostgreSQL source and native equivalence checks.
- [Backup/restore](../operations/backup-restore.md): archive validation, isolated restore, and crash behavior.

The runtime image excludes the fixture controller. Synthetic providers and fake credentials do not establish live provider access.

Reports belong to the check that produced them. Passing worker unit tests alone does not establish a full browser or Telegram workflow.
