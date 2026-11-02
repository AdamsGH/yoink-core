# Insight runtime and configuration

The server constructs the native Insight service and shares it with HTTP, Telegram, and enabled Inbox integration. It uses the shared database, identity store, confined executor, and delivery journals.

No separate Python AI service starts in the native deployment.

## Settings

Every setting below has the `YOINK_RUST_` prefix. The binary does not load `.env`.

| Suffix | Meaning |
| --- | --- |
| `INSIGHT_ENABLED` | Enable standalone Insight. Default `false`. |
| `INSIGHT_GATEWAY_BASE`, `INSIGHT_GATEWAY_KEY` | Operator gateway route. |
| `INSIGHT_OPENROUTER_BASE`, `INSIGHT_OPENROUTER_KEY` | Operator OpenRouter route. |
| `INSIGHT_MODE` | `auto`, `a2a`, or `openrouter`. |
| `INSIGHT_MODEL` | Normal completion model. |
| `INSIGHT_SEARCH_MODEL` | Independent research model. |
| `INSIGHT_GEMINI_KEY`, `INSIGHT_GEMINI_MODEL` | Gemini configuration. |
| `INSIGHT_AI_PROXY` | Explicit SOCKS5 AI tunnel. |
| `INSIGHT_DEFAULT_LANG` | Response language. Default `en`. |
| `INSIGHT_TRANSCRIPT_LANGUAGES` | Ordered transcript-language JSON array. Default `["en","ru"]`. |
| `INSIGHT_RESPONSE_MAX_BYTES` | Source/provider response bound. Default 1048576, maximum 4194304. |
| `INSIGHT_CONTENT_MAX_CHARS` | Extracted content bound. Default 40000. |

`INSIGHT_OPENROUTER_PROXY` is a compatibility fallback for the AI proxy setting. The native transport requires `socks5`, not HTTP CONNECT or `socks5h`.

A configured model is not a promise that a provider supports it. The service does not invent a usable default model when the configuration leaves it empty.

The [typed loader](../../../backend/crates/yoink-server/src/insight_config.rs) defines the complete limits, defaults, and validation.

## HTTP and authority

Insight routes mount at `/api/v1/insight`. Shared authentication supplies the actor. The product rechecks current bans, grants, settings, and credential revisions.

Read and write OAuth flows use separate application configuration. OAuth status does not expose device or access tokens.

Provider requests cannot use a token after replacement, expiry, revocation, or loss of authority. Protected persistence rechecks those conditions after external I/O.

## Lifecycle

Launcher preflight must complete before readiness. Startup cancellation kills and reaps the owned preflight child before database teardown.

Shutdown closes admission, cancels work, drains text delivery, joins OAuth/content cleanup, and releases quota tracking before database close.

The request and OAuth budgets, plus database acquisition allowances, must fit strictly below shutdown. Default standalone Insight requires an explicit longer shutdown budget. Compose supplies 150 seconds.

## Network and content boundaries

Content fetching uses the confined worker. It validates source destinations and redirects. GitHub read credentials apply only to their intended origin.

Personal provider endpoints use guarded public-address resolution. Operator gateways have a separate trusted configuration boundary.

Independent research uses real provider citations and excludes the reviewed source from corroboration. It is not equivalent to checking every generated statement.
