# Native Inbox

The crate owns ingestion, classification, saved items, GitHub state, and durable product effects.

The shared server composes this application crate. It is not a standalone server and does not initialize a separate database.

- [Product guide](../../../docs/features/inbox/README.md)
- [Runtime architecture](../../../docs/architecture/overview.md)
- [Development checks](../../../docs/development/verification.md)

Use the existing backend recipes for focused checks:

```sh
just --justfile backend/justfile test-crate yoink-inbox
just --justfile backend/justfile lint-crate yoink-inbox
```
