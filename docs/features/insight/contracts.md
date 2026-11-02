# Insight service contracts

The native Insight service combines content extraction, prompt selection, provider routing, quota accounting, cache identity, and delivery authorization.

## Prompts and cache

About and summary require transcript content. TLDR supports configured aliases, wildcard domain bindings, focus, language, and user prompt overrides.

Cache identity includes the effective content, prompt, model, research mode, and private provider context. A cached result must not cross user or credential boundaries.

Current authority applies to cache reads as well as fresh requests.

## Provider behavior

Gateway/OpenRouter routing and personal provider settings have separate authority. A personal provider failure does not permit an undisclosed fallback to operator credentials.

Model catalogues are bounded remote data, not evidence of live completion success. Failed probes must not publish unverified replacement credentials.

Credential replacement uses revision checks. A slow probe cannot overwrite a newer credential or restore a revoked grant.

## Streaming

The streaming path consumes actual provider events with bounded input and awaited delivery. It checks cancellation and current authority during processing.

A cached response does not generate synthetic deltas. Buffered HTTP responses remain a separate contract.

Partial external delivery prevents fallback that could repeat content. Unknown send/edit effects stay uncertain until the durable ledger supplies valid recovery evidence.

## Research

Research must contain real citations. The source under review cannot serve as its own independent corroboration.

Failure to obtain citations is a research failure, not successful verification.

## OAuth and shared use

Device flows handle cancellation, expiry, generation replacement, slow polling, bounded responses, and post-I/O authority checks.

Inbox reuses the same authorized completion/content interfaces. It does not bypass personal provider restrictions or the shared shutdown lifecycle.

See [runtime configuration](runtime.md), [diagnostics](diagnostics.md), and [focused contracts](../../development/fixtures/insight-contracts.md).
