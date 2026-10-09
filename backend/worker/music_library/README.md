# Confined Music provider functions

Parsers, search adapters, similarity helpers and artist functions were retained
from this repository's `plugins/yoink-music/src/yoink_music` at baseline
`e1aaa6c`. No plugin loader, Telegram handlers, legacy configuration, database
access or service startup is retained. No new license grant is made for these
repository-origin files. Dependency licenses remain those of upstream packages.

Imports use worker-local bounded transports. Anonymous Yandex API calls replace
the SDK. Metadata extraction uses the worker-local yt-dlp wrapper. Requests-based
ytmusicapi traffic uses bounded responses, configured proxies and existing
DNS/socket guards. Curl traffic pins direct DNS and handles explicit redirects.
No TLS or private-address exemptions are added. Rust owns orchestration,
credential/config admission, policy, caches, analytics, cards and delivery.
