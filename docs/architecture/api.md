# HTTP API

The native API uses `/api/v1` for authenticated browser requests. Product routes share the same current identity and authorization boundary.

## Authentication

`POST /api/v1/auth/token` accepts JSON containing Telegram WebApp `init_data`. The service validates the bot signature and timestamp before issuing a JWT.

Send the token as `Authorization: Bearer TOKEN`. The service reloads current database roles and bans. Token role claims are not authority.

Development impersonation exists only when `YOINK_RUST_DEV_AUTH_ENABLED=true`. It selects an existing unblocked account and cannot elevate its role. Keep it disabled on exposed instances.

## Main route families

| Prefix | Purpose |
| --- | --- |
| `/api/v1/users` | Profiles, account administration, and activity. |
| `/api/v1/groups` | Registered groups, topic policies, and membership administration. |
| `/api/v1/settings` | Core language and theme preferences. |
| `/api/v1/bot-settings` | Bot configuration and feature/tag settings. |
| `/api/v1/dl` | Download settings, history, cookies, and content rules. |
| `/api/v1/music` | Music account statistics. |
| `/api/v1/stats` | Group analytics and related product operations. |
| `/api/v1/insight` | AI settings, prompts, aliases, providers, and usage. |
| `/api/v1/inbox` | Saved content, jobs, folders, Stars, rules, and teams. |

This table groups routes. It is not an endpoint-by-endpoint authorization matrix.

## API description

`GET /api/v1/_meta/openapi.json` returns the checked-in OpenAPI 3.1 description. It requires a currently authorized admin or owner and uses `Cache-Control: no-store`.

The native service does not provide the legacy Swagger UI at `/docs`. The nginx route alone does not create that UI.

[configuration-openapi.json](../../backend/crates/yoink-http/src/configuration-openapi.json) is the machine-readable description. [Contract tests](../../backend/crates/yoink-http/tests/openapi-contracts.test.mjs) check it against route sources.

## Settings concurrency

Downloader GET/PATCH uses ETags and optional revision preconditions. The Mini App submits `If-Match` with changed writable fields and handles HTTP 409 explicitly.

Core settings and other product APIs have their own contracts. Do not assume every PATCH shares the downloader revision format.

## Health and metrics

| Path | Meaning |
| --- | --- |
| `/health/live` | Process liveness. |
| `/health/ready` | Startup/dependency readiness. Failure returns HTTP 503. |
| `/health` | Compatibility health body, including degraded state. It can return HTTP 200 while degraded. |
| `/api/v1/system` | Runtime version and diagnostic state. |
| `/metrics` | Native metrics endpoint. |

Use readiness rather than the compatibility health status code to gate deployment.

## Other interfaces

The M2M API uses `X-Api-Key` and the `/api/internal/v1` prefix. Key scopes, revocation, and expiry apply independently of browser authentication. A creator's later role change does not revoke a separate machine credential.

The optional [user-session adapter](../operations/user-session.md) supplies protected topic, forum, and message metadata operations.

See [authorization](access-control.md) for role precedence, feature access, bans, and route-specific constraints.
