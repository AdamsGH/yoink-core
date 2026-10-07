# User-mode Telegram session

The native runtime uses a separately configured tdlight HTTP service. It does not log in to Telegram or create session files. Without configuration, status reports `available: false` and protected session operations return an unavailable error.

## Configuration

The server reads these settings with the `YOINK_RUST_` prefix. It does not load an env file.

| Setting | Default | Maximum |
| --- | --- | --- |
| `USER_SESSION_URL` | Unconfigured | Trusted HTTP or HTTPS origin |
| `USER_SESSION_TOKEN_FILE` | Unconfigured | Absolute file path |
| `USER_SESSION_TIMEOUT_SECS` | 30 | 120 |
| `USER_SESSION_TOKEN_TIMEOUT_MS` | 1000 | 10000 |
| `USER_SESSION_RESPONSE_BYTES` | 4194304 | 16777216 |
| `USER_SESSION_TOKEN_BYTES` | 4096 | 16384 |
| `USER_SESSION_SCAN_PAGES` | 100 | 100 |
| `USER_SESSION_SCAN_TOPICS` | 10000 | 10000 |

Set URL and token file together. The origin cannot contain credentials, a path prefix, a query or a fragment. The token file must belong to the runtime UID, have no group or other permissions, and have one hard link. Every path component must be free of symlinks. Mount the file read-only into the runtime container.

The client reads the token before every request. Token rotation does not require a restart. Reads have a deadline, a byte limit and one concurrent blocking reader per client. Redirects and environment proxies are disabled. Public errors omit token paths, upstream bodies and transport URLs. The tdlight protocol places the token in `/user{token}/method`; operators must keep upstream access logs private.

## HTTP contracts

The composition root registers these routes once, with authentication and runtime readiness checks:

- `GET /api/v1/threads/status`
- `POST /api/v1/threads/scan/{group_id}`
- `GET /api/v1/forum/topics/{chat_id}`
- `GET /api/v1/forum/topics/{chat_id}/{thread_id}`
- `GET /api/v1/forum/topics/{chat_id}/{thread_id}/link`
- `GET /api/v1/forum/search/{chat_id}`
- `GET /api/v1/forum/history/{chat_id}`
- `GET /api/v1/messages/{chat_id}/{message_id}/viewers`
- `GET /api/v1/messages/{chat_id}/{message_id}/link`
- `GET /api/v1/messages/{chat_id}/{message_id}/read-date`
- `GET /api/v1/messages/{chat_id}/{message_id}/thread`
- `GET /api/v1/messages/{chat_id}/by-date`

Each handler checks current database authority through `configuration_authorized`. Status requires an administrator. Session proxy operations and scan require the owner. Scan also requires a negative group ID. An unavailable adapter returns HTTP 503 after authorization and input validation. An invalid scan ID returns HTTP 422.

A configured scan verifies the group before requests, rechecks authority before every page, and rechecks authority inside the write transaction. It updates discovered topic names without deleting manual policies or changing existing enabled flags. The existing schema represents all scan data, so this integration needs no migration.

## Shared client

`yoink_user_session::Client` owns transport and typed DTOs. Its member API is `get_chat_members(chat_id, filter, offset, limit)`. `ChatMemberFilter` supports `Members`, `Administrators`, `Banned` and `Restricted`. The page limit is at most 200.

A Stats adapter must check offset arithmetic, preserve failures instead of treating them as empty pages, and map missing tokens or upstream authorization failures to an actionable unavailable result. Client `available()` checks the token asynchronously. A synchronous capability flag does not prove that a session request will succeed.

## Verification

Run `just test-user-session` from `backend/` for client, HTTP, identity and server contracts. HTTP fixtures use the production composition factory and a loopback tdlight service. They cover missing configuration, authorization, upstream failures, atomic topic-name updates and runtime readiness. Run `just format-user-session` for the affected Rust crates.

Real tdlight connectivity and the Stats runtime bridge require separate integration acceptance. Unit and loopback checks do not establish production session availability.
