# Insight changes

- Use the existing `InsightRuntime` and shared authenticated service. Historical integration patches are not installation instructions.
- Reuse the same authorized content/completion interfaces in Inbox.
- Keep content fetching in the confined worker. Do not add host Python or an unconstrained Rust URL fetcher.
- Preserve provider, credential-revision, quota, and current-ban checks around external I/O.
- Keep BYOK separate from operator fallback authority.
- Do not fallback after partial external delivery or synthesize streaming deltas for cached text.
- Register schema changes through the existing native migration runner.
- Read [service contracts](../../features/insight/contracts.md) and [runtime ownership](../../features/insight/runtime.md) before changing composition.
