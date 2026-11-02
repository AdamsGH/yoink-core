# Native Music

The crate owns music use cases, fresh access checks, cards, cache identity, and audio effect persistence.

The shared server composes this application crate. It is not a standalone server and does not initialize a separate database.

- [Product guide](../../../docs/features/music.md)
- [Runtime architecture](../../../docs/architecture/overview.md)
- [Development checks](../../../docs/development/verification.md)

Use the existing backend recipes for focused checks:

```sh
just --justfile backend/justfile test-crate yoink-music
just --justfile backend/justfile lint-crate yoink-music
```
