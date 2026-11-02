# Insight focused verification

Focused tests validate product contracts before actual-process acceptance.

## Commands

Run from the repository root:

```sh
just --justfile backend/justfile test-insight
just --justfile backend/justfile check-insight
```

These recipes cover module tests and scoped checks. They do not publish images or contact production providers.

## Contracts

Tests cover provider wire formats, research citations, personal credential probes, route replacement, OAuth lifecycle, prompt/alias behavior, cache identity, and authorization.

Inbox integration tests exercise the shared content/completion interfaces with synthetic inputs and temporary databases.

The [trust fixtures](../../../backend/crates/yoink-insight/tests/fixtures/trust/README.md) document their source and intended assertions.

## Acceptance boundary

Module tests do not establish real worker confinement, Telegram streaming, browser behavior, or shutdown receipt recovery.

Use [the actual-process fixture](insight.md) for those checks. Keep fixture images matched to the runtime source.

Historical worktree test totals are not a current verification result. Record the command and outcome from the checkout under review.
