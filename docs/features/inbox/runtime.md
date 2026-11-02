# Inbox runtime

The server owns Inbox's durable job loop, confined content executor, GitHub client, shared Insight integration, and Telegram notification adapter.

HTTP and Telegram use the same database and identity authority. The runtime does not create a second database owner.

## Configuration

All settings use the `YOINK_RUST_` prefix.

| Suffix | Default |
| --- | --- |
| `INBOX_ENABLED` | `false` standalone. |
| `INBOX_JOB_TIMEOUT_SECS` | `60`. |
| `INBOX_DRAIN_TIMEOUT_SECS` | `5`. |
| `INBOX_LEASE_SECS` | `120`. |
| `INBOX_TEXT_TIMEOUT_SECS` | `5`. |
| `INBOX_EXECUTOR_TIMEOUT_SECS` | `20`. |
| `INBOX_SOCKET_TIMEOUT_SECS` | `15`. |
| `INBOX_COMPLETION_TIMEOUT_SECS` | `30`. |
| `INBOX_GITHUB_TIMEOUT_SECS` | `15`. |
| `INBOX_CONTENT_MAX_CHARS` | `40000`. |
| `INBOX_WORKSPACE` | `/tmp/yoink-inbox`. |

The [typed loader](../../../backend/crates/yoink-server/src/inbox_config.rs) defines all bounds and validates relationships between stage, job, lease, and shutdown budgets.

GitHub endpoint overrides are operator-trusted configuration. They must not come from user input.

## Jobs and shutdown

Admission records durable intent. Claims and completion use lease fences. A stale worker cannot publish a classification after a newer owner or cancellation.

Shutdown cancels and drains owned jobs before database close. A whole-job deadline counts as failure and consumes an attempt. Owning shutdown interruption follows the separate cancellation/recovery contract.

No loop is intentionally detached. Database failure cannot promise successful lease release.

## Shared services

When full Insight is enabled, Inbox shares its authorized content/completion service. Completion-only integration remains distinct from exposing standalone Insight routes.

Telegram notification recovery uses the same database as text delivery. Group policy must match the shared Telegram adapter.

See [contracts](contracts.md), [content processing](content.md), [Telegram delivery](telegram.md), and [isolated acceptance](../../development/fixtures/inbox.md).
