# Insight

Insight provides AI summaries, configurable prompts, model selection, and optional independent research.

## Commands

```text
/about YOUTUBE_URL
/summary YOUTUBE_URL
/tldr URL
```

About and summary use transcript content. TLDR can use supported fetched content, aliases, domain bindings, and saved prompt preferences.

The Mini App provides settings, prompts, aliases, provider controls, usage views, and GitHub authorization. Current feature grants and bans apply to commands and API calls.

## Providers

The native service supports configured gateway and OpenRouter routes, plus supported personal provider settings. BYOK means a user's own provider key.

Provider catalogue entries do not prove live model availability. Research must return actual citations. A source URL cannot corroborate its own claims.

## Configure

The native setting names use `YOINK_RUST_INSIGHT_*`. The runtime requires an explicit SOCKS5 AI proxy. Legacy lowercase Python configuration does not configure the native service.

See [runtime configuration](runtime.md) for provider settings and deadlines. Keep API keys private. Redacted settings responses do not return stored credentials.

## Delivery and recovery

Streaming responses use actual provider deltas. Cached responses do not simulate streaming.

After partial delivery, the service must not switch providers and repeat the answer. Unknown external effects require reconciliation rather than automatic resend.

See [service contracts](contracts.md), [diagnostics](diagnostics.md), and [isolated fixtures](../../development/fixtures/insight.md).
