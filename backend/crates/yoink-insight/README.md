# Native Insight

The crate owns prompts, summaries, provider routing, OAuth, quota accounting, and product APIs.

The shared server composes this application crate. It is not a standalone server and does not initialize a separate database.

- [Product guide](../../../docs/features/insight/README.md)
- [Runtime architecture](../../../docs/architecture/overview.md)
- [Development checks](../../../docs/development/verification.md)

Use the existing backend recipes for focused checks:

```sh
just --justfile backend/justfile test-crate yoink-insight
just --justfile backend/justfile lint-crate yoink-insight
```
