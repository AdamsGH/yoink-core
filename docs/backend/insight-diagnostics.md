# Insight diagnostics

Native Telegram Insight records structured, correlated metadata at INFO level.
Each accepted request has a UUID plus its chat and input message IDs. Records cover
source/research/generation phases, cached responses, provider attempts, draft
previews, final sends, confirmed receipts and failed finalization.

Provider records include the normalized completion reason, delta count, wire and
output byte counts, elapsed time and time to first delta. Cancellation and timeout
are distinct outcomes. OpenAI `length`, Anthropic `max_tokens` and Gemini
`MAX_TOKENS` are incomplete responses, not successful completions. An interrupted
generation is not cached or silently retried after output has begun.

Draft records distinguish success, unsupported methods, transport errors, timeout,
invalid responses and API errors. HTTP status and numeric API error codes are
retained; descriptions and credential-bearing URLs are not. Drafts are ephemeral.
Only a confirmed final Bot API send proves delivery. Final delivery errors are
recorded, without unsafe retries of ambiguous sends.

## Collect a report

From the repository root:

```sh
just -f backend/justfile insight-diagnostics
just -f backend/justfile insight-diagnostics 24h REQUEST_UUID
```

The report contains current container health, grouped request events and an outcome
classification. It reads at most 20000 retained lines from the current container.
Requests with no terminal event are classified as in progress or missing a log
tail, not automatically as failures. Keep chat IDs, message IDs and request IDs
private even though the report excludes secrets and message contents. For a file:

```sh
umask 077
just -f backend/justfile insight-diagnostics > insight-report.json
```

The collector outputs only allowlisted metadata. It does not emit environment
values, provider endpoints, keys, request text, generated text or raw Docker errors.
No HTTP-client debug logging is required or enabled.

## Inspect older deliveries

Old durable delivery records can establish receipt state and whether the original
request asked for brevity. They cannot reconstruct missing SSE completion reasons.
The collector accepts only inactive restores under `data/native-restore`, never
the active database. Existing restores can be inspected without pausing production:

```sh
just -f backend/justfile insight-diagnostics 24h '' latest
just -f backend/justfile insight-diagnostics 24h '' latest 'non-secret reply fragment'
just -f backend/justfile insight-diagnostics 24h REQUEST_UUID latest
```

`latest` means the latest named inactive restore, not a live snapshot. Its timestamp
and migration version are included in the report. With a UUID, only matching
chat/message deliveries are selected. This needs a retained UUID-to-message mapping;
old snapshots alone do not contain UUIDs. To obtain a fresh copy, run the existing
offline backup workflow. It gracefully pauses and resumes the application:

```sh
flock --exclusive --nonblock .pi/native-maintenance.lock just backup run
just backup restore latest
just -f backend/justfile insight-diagnostics 24h '' latest
```

Snapshot queries run as UID 10001 in a read-only, network-disabled container, using
the running immutable image. No host Python or permission relaxation is needed.
Symlink snapshots and snapshots with a WAL are rejected. Reply hashes and lengths
are emitted instead of content. A match fragment is used internally, not printed.
Container replacements may remove old Docker logs; collect a private report before
replacement when preserving an investigation. A database restore does not contain
provider diagnostic events from Docker logs.

## Verify

```sh
just -f backend/justfile test-insight-diagnostics
just -f backend/justfile test-crate yoink-insight
just -f backend/justfile test-crate yoink-telegram insight_runtime_tests
```
