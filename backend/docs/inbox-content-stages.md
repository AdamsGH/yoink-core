# Confined Inbox content primitives

`yoink_inbox::content_stages::ContentStages` implements `enrich::Stages`.

```rust,ignore
let stages = ContentStages::new(executor, operator_proxy_url, job_cancel);
let content = FallbackContent {
    stages: &stages,
    github: &github,
    insight: &insight,
    max_chars,
};
```

Construction takes an `Executor`, `Option<String>` operator HTTP(S) proxy and
`tokio_util::sync::CancellationToken`. No user token, cookies or OAuth enter
content stage requests. `user` in `Stages::fetch` is intentionally unused;
resolve operator proxy policy before constructing the adapter. The adapter
borrows for each call, creates a fresh operation ID and uses a child cancellation
token. `structured` retains the trait's native JSON implementation.

## Executor configuration contract

Use `/usr/local/bin/yoink-worker-launcher`, empty args, and an existing absolute
workspace root. The launcher creates no source search paths from the workspace;
Python `main.py` adds only its immutable source directory. Stage execution shares
media's process-group kill/reap, dropped-future cleanup, bounded stdin/stdout/
stderr, workspace accounting and executor deadline. `Executor::execute_content`
is also callable directly with a typed protocol `ContentRequest` and cancellation
token.

A cloned media executor works, but independently configure an Inbox executor to
keep Inbox enabled when Telegram/downloads are disabled and avoid long media
per-stage deadlines. Suggested limits: 30-second executor deadline, 20-second
socket timeout, processing timeout no greater than executor timeout, 1 MiB stdout,
1 MiB artifact limit, 2 MiB workspace limit, one artifact maximum. These are
configuration suggestions, not defaults or server runtime edits. Content leaves
no artifacts; any returned artifacts are rejected. Coordinator owns whole-job
budget/lease validation, image construction and loop composition.

Content ignores `allowed_private_origins`: no private source exemptions are
accepted. `allowed_proxy_origins` may authorize an operator proxy but never a
source. Proxy requests use the existing DNS-pinned CONNECT relay. Direct requests
pin validated DNS results and preserve HTTPS hostname verification. Every redirect
is validated. Provider requests revalidate the embedded original destination,
including redirects into providers, and nested provider forwarding is denied.

## Bounds and failures

Protocol v2 adds `content_get`, `content_readability`, `content_trafilatura`, typed
`content_completed`, and fixed `content_denied`. Requests reject unknown fields.
HTML/result text is capped at 128 KiB UTF-8, titles at 2000 bytes, URLs at 8192
bytes and frames at the existing 1 MiB ceiling. GET permits only the exact Inbox
Accept/X-No-Cache/X-Return-Format values. There are at most six HTTP attempts.
The transport requests identity encoding and rejects non-identity content
encoding **before reading/decompressing**; compressed responses are not inflated.
Declared lengths, actual body lengths, and deadline are bounded. This deliberately
fails closed on servers ignoring identity encoding rather than exposing automatic
unbounded decompression through requests.

Network denials map to existing `Error::Forbidden`; every Rust fallback fetch
propagates that error instead of converting it into a miss. Ordinary transport or
extractor failures remain `Error::External` and Rust owns fallback order, metadata,
provider chrome and final truncation. Worker diagnostics never contain exception
strings, URLs or credentials. The seeded standalone v1 `content_main.py` remains
available for future Insight integration and is not used by Inbox stages.

## Verification and image handoff

- `just test-inbox-content` and `just check-inbox-content`: focused Rust crates,
  resource preflight, one local target, debug=0, incremental=false, jobs=1.
- `just test-content-primitives IMAGE_ID`: safety tests without new dependencies.
- `just test-content-worker IMAGE_ID content_test_fetch.py`: seeded contracts.
- `just test-content-worker IMAGE_ID test_content_stage.py`: all stage tests.
- `just test-content-worker IMAGE_ID 'test_*.py'`: existing worker/media regressions.
- `just build-content-worker`: builds the pinned Rust launcher and worker without building the whole server.
- `just test-content-launcher IMAGE_ID`: exercises private-destination denial and both real extractors through the fixed launcher in hardened disposable containers, without source mounts.

All Python runs in disposable Docker containers with source mounted read-only.
Coordinator must build the modified worker-runtime Dockerfile containing exact
`readability-lxml==0.8.4.1` and `trafilatura==2.0.0`, then rerun
`DependencyExtractionTests.test_readability_pinned_dependency` and
`DependencyExtractionTests.test_trafilatura_pinned_dependency`. They check the exact installed dependency versions, not substitute parsers. The rebuilt worker passed 128 tests; the separate content-worker image also passed fixed-launcher denial and both extraction checks. Trafilatura uses its public `prune_xpath` option to remove navigation and footer elements without filtering article prose or duplicating paragraphs.
Production provider networking and full native composition remain coordinator
integration gates.

## Cancellation

Executor cancellation maps to `Error::Cancelled`. Optional fetch, readability and trafilatura propagate it without trying another provider. The native job worker restores the interrupted admission's attempt and requeues it under the live lease token, without changing item, LLM or stars status. Ordinary failures still consume the configured retry budget.

The composition root must cancel each job's concrete adapters, await child-process cleanup, and keep the whole-job deadline plus cancellation drain below its lease duration. GitHub and completion calls also need bounded cancellation. Server composition and actual-runtime shutdown/restart acceptance remain required; primitive tests alone do not establish these properties.
