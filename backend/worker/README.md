# Confined Python worker

This directory implements media, content, search, Music, and Inbox worker tasks. Rust services launch it through the fixed Landlock launcher.

Each process handles a typed attempt in a private workspace. The worker does not start a bot, API server, or database owner.

See [worker development](../../docs/development/workers.md) for transports, limits, networking, and container-backed tests. The [protocol reference](../../docs/architecture/worker-protocol.md) describes request and artifact validation.

[Music provider functions](music_library/README.md) retain their source and licensing context.
