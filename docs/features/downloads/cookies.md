# Cookies and YouTube authentication

Cookies are private credentials. The native service stores personal cookies separately from the shared pool and rechecks current authority for reads and mutations.

Cookie lists return metadata, not contents. Request-body user IDs and JWT role claims do not select ownership.

## Personal cookies

Strict Netscape validation applies to ordinary uploads. Replacement retains relevant personal metadata and advances credential identity.

Queued work holds versioned references. Replacement or deletion invalidates stale references instead of silently selecting another cookie.

Personal capacity has configurable domain, actor-byte, and global-byte limits. Imported over-limit data can shrink or be removed without permitting new growth.

## Browser tokens

`/cookie token` issues a ten-minute single-use token. The browser extension submits it without a JWT.

Consumption and cookie writes commit together. Validation, capacity, or current-authorization failure rolls back the transaction and preserves the token.

Tokens expire when the current time reaches their deadline. Explicit revocation removes the actor's outstanding tokens.

The token response supplies the submission endpoint. It does not invent a mobile upload page.

## WebApp origin

`YOINK_RUST_COOKIE_WEB_ORIGIN` enables a link to the existing `/cookies` page. It requires an HTTPS origin without credentials, query, fragment, or a non-root path.

The native API does not serve the SPA. The operator must route the frontend and API at the configured origin.

There is no implemented `/cookie-sync` page. See the [browser extension](../../../browser-extension/README.md) for the supported submission workflow.

## API routes

Base path: `/api/v1/dl/cookies`.

| Method | Suffix | Purpose |
| --- | --- | --- |
| GET, POST | Base path | Personal metadata and validated personal upsert. |
| POST | `/upload` | Personal upload with authentication-cookie checks. |
| DELETE | `/{domain}` | Personal domain removal. |
| DELETE | `/by-id/{id}` | Personal owned-row removal or authorized administration. |
| GET | `/all`, `/pool` | Authorized administrative metadata. |
| POST | `/pool` | Authorized bounded pool update. |
| DELETE | `/pool/{id}` | Authorized pool removal. |
| POST | `/pool/refresh-labels` | Live account-label refresh. |
| POST | `/{id}/validate` | Owned or administrative live validation. |
| POST, DELETE | `/token` | Issue or revoke personal sync tokens. |
| POST | `/submit` | Token-authorized browser cookie submission. |
| POST | `/yttv/start` | Start an owned device session. |
| GET | `/yttv/poll/{session}` | Poll the owned session. |

The [HTTP router](../../../backend/crates/yoink-http/src/cookie_web.rs) defines route ordering and response handling. No HTTP cookie-content export is exposed.

## Pool access

Inherited pool metadata requires current pool authority and a valid row. Pool opt-in is separate from permission.

Management operations recheck current bans, roles, and ownership under the protected write transaction.

Pool storage has its own finite bound. A personal mutation must not rewrite unrelated pool rows.

## YouTube OAuth

YouTube TV credentials use a typed versioned reference. Rust owns secret resolution, refresh, and current-authority checks.

Ordinary cookie upload does not accept OAuth credential JSON. Secret resolution is not a frontend content endpoint.

Provider calls happen outside the database guard. Protected writes recheck the expected reference and current authority afterward.

The [combined YouTube service](../../operations/youtube-services.md) supplies the fixed Unix adapter. Synthetic device responses do not prove live account authorization.

## Polling and shutdown

| Setting | Default |
| --- | --- |
| `YOINK_RUST_COOKIE_OAUTH_POLL_BUDGET_SECS` | `120`. |
| `YOINK_RUST_COOKIE_OAUTH_RESULT_RETENTION_SECS` | `1800`. |
| `YOINK_RUST_COOKIE_OAUTH_MAX_INFLIGHT` | `128`. |
| `YOINK_RUST_COOKIE_OAUTH_CLEANUP_INTERVAL_SECS` | `60`. |

The accepted-work budget covers database waits, provider I/O, and persistence. Retries do not extend staged-result retention.

Shutdown closes poll admission and drains accepted tasks before store close. Its budget must exceed polling by more than five seconds.

Forced termination or prolonged database unavailability can still lose an unstaged provider response.

## Verification

`test-cookie-web-regression` checks storage, HTTP, Telegram, and lifecycle contracts. `test-personal-controls-native` exercises actual token issuance, submission, and cookie-authenticated extraction on disposable state.

These checks do not establish the public reverse proxy, every browser flow, or live provider authorization.
