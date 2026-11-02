# Inbox content processing

Inbox uses confined content stages for fallback extraction. Structured GitHub extraction and authorized Insight completion remain separate adapters.

## Execution

`ContentStages` sends typed requests through the fixed executor. Each call receives a fresh operation identity and child cancellation token.

Content stages do not receive personal cookies, user OAuth credentials, or arbitrary executable paths. The operator proxy has separate trusted configuration.

Requests return bounded text, not deliverable media artifacts. Unexpected artifacts fail validation.

## Network policy

Content fetching validates DNS and redirects. Private source exemptions are not accepted for these content stages.

An approved proxy origin does not authorize private source destinations. Direct HTTPS retains hostname verification and pinned address checks.

Provider forwarding must revalidate the original destination. Nested forwarding cannot bypass source policy.

## Limits and fallback

The server defines stage deadlines, response bounds, extracted-character limits, and workspace limits. Stage budgets must fit the enclosing job budget.

Fallback thresholds distinguish useful text from raw HTML or provider interface chrome. The pipeline must not report empty or malformed output as successful content.

GitHub extraction uses bounded fixed-origin raw/API reads and scoped credentials. It preserves repository identity and source metadata.

## Cancellation

The executor owns process groups and workspace cleanup. Cancellation stops and reaps the active worker before returning.

Shutdown also drains the job owner. A forced abort does not establish that an external provider or delivery request had no effect.

The [worker protocol](../../architecture/worker-protocol.md) and [Inbox fixture](../../development/fixtures/inbox.md) describe the implementation and verification boundaries.
