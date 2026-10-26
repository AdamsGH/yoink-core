# Stats full-text search

## Approved search contract

The migration uses a separate Tantivy 0.26.2 index. Native Turso remains the authoritative store. The implementation does not enable Turso's experimental FTS or replace full-text matching with SQL `LIKE`.

The previous Stats implementation used PostgreSQL `to_tsvector('simple', text || ' ' || caption)` and `plainto_tsquery('simple', query)`. Search only filtered rows. It did not stem Russian or English words, remove stopwords, rank results, or interpret quoted input as a phrase. Eight analytics methods consume that filter: counts, hours, days, week, history, words, random quotes and ECDF.

The user approved a new Tantivy tokenization contract rather than exact compatibility with PostgreSQL's text parser. Version 1 splits text into Unicode alphanumeric runs and lowercases tokens. It indexes both message text and caption. Distinct query terms combine with AND, including when matching terms occur in different fields. There is no stemming, accent folding, substring matching, prefix expansion, fuzzy matching or relevance ranking. `ё` and `е` remain distinct. Words such as `and`, `or` and `the` are ordinary searchable words. Quotes, minus signs and other punctuation do not introduce query operators.

An absent or empty query disables the text filter. Nonempty input producing no tokens, including whitespace or punctuation alone, matches nothing. This distinction preserves the existing Python `if lquery` behavior. The tokenizer drops tokens of 2048 bytes or longer. Query and pagination bounds fail with explicit errors instead of silently truncating requests.

## Deliberate differences

PostgreSQL's default parser has special rules for hostnames, email addresses, paths, signed numbers, HTML tags and hyphenated compounds. Tantivy uses word boundaries for those inputs instead.

| Query | PostgreSQL behavior | New behavior |
| --- | --- | --- |
| `foo-bar` | Requires the compound lexeme as well as its components | Requires `foo` and `bar`, including in a message containing `foo bar` |
| `example.com` | Matches the hostname lexeme | Requires `example` and `com`, including separate words or parts of an email address |
| `mail@example.com` | Matches the email lexeme | Requires `mail`, `example` and `com` |
| `-12.5` | Matches the signed decimal lexeme | Requires `12` and `5` |
| `b` in `<b>hidden</b>` | Tags are not searchable words | Tag names are searchable words |
| `ракета-носитель` | Requires the compound lexeme | Requires the two component words |

These are approved behavior changes, not compatibility fixes. The shared corpus at `backend/scripts/stats-search-fixture/cases.json` contains 30 messages and 33 queries. Eight query results differ deliberately. The PostgreSQL oracle runs the real Stats trigger after all 54 Alembic migrations in an isolated container. Rust tests exercise the new contract through the native search service against the other expected result set.

## Source and index lifecycle

Native migration V3 adds search state and a dirty-message queue. Ordinary insert, update and delete triggers on `stats_messages` record pending work in the same transaction as the source change. Updates to text, caption and filter fields all invalidate the indexed document. Deletions leave tombstones. Existing messages are queued when V3 is applied, including databases created before search support.

The search service refreshes the index before returning results. Source reads, index refresh and result hydration share a serialized database snapshot. Tantivy work runs outside Tokio's async executor threads. Index commit and reader reload precede acknowledgement of the dirty queue. If acknowledgement fails, replay remains idempotent. A source rollback cannot publish a new message to the index.

Tantivy's atomic commit payload binds the cache to the native database's random identity, canonical path, file device/inode, acknowledged source revision and tokenizer version. It is not a separately written sidecar. Cancelled updates mark the live service for rebuilding. A commit ahead of native acknowledgement is detected again after reopening. An absent, incompatible or stale index requires a rebuild from native message rows. Rebuilds read bounded batches. Native source data is never reconstructed from the index. An index error must not cause fallback to stale results or to a different search implementation.

## Integration and operational limits

`yoink-stats-search` supplies the repository-level search service. The full Stats Telegram commands, HTTP routes and analytics aggregations are separate product-module work. This slice does not expose them or switch the deployed Python service.

Every search supplies a nonzero chat scope and may further filter sender, inclusive UTC date bounds and exact message type. Native timestamps are signed UTC microseconds, including dates before 1970. Results order by date descending, then internal ID descending. Totals count every matching document, without a global hit cap. Page size is at most 500 and offset at most 100,000. Page size zero, including the default request, returns only the count. Queries are at most 16,384 UTF-8 bytes and 64 distinct tokens, and message-type filters at most 128 bytes. Search scope is not authorization. Future adapters must check the caller's current access to the requested chat before invoking this trusted repository API.

The native database and derived index both contain private message information. Keep them in private directories outside the media worker's accessible roots. One server process owns the native file. Preserve that ownership when opening a search service. Index files are disposable but must not be removed while the service holds them open.

A consistent backup of the native database is sufficient to rebuild search. Closing `Store` alone does not checkpoint its WAL: do not copy only the main file after close. The restore-identity tests explicitly verify a successful `wal_checkpoint(TRUNCATE)` with no concurrent writer before closing and replacing their disposable source file. Those are source-identity fixtures. Use the separate [backup/restore utility](backup-restore.md) for an operator snapshot. An independently copied index is not an authoritative backup. Open a restored database as a new file and rebuild or validate its derived index before serving results. In-place replacement of a live native file is not supported. A corrupt index that cannot be opened requires an explicit fresh private index directory and rebuild. Do not delete unrelated files to recover it.

Full rebuilds temporarily occupy the serialized database connection and consume index disk space. Ordinary searches apply queued changes rather than rebuilding the entire dataset. Dataset-scale throughput, disk-exhaustion behavior and production sizing are not established by the synthetic tests. The backup/restore suite also verifies reconstruction of an existing index from a restored database. Production cutover remains a separate gate.

## Verification

From `backend/`:

```sh
just test-stats-search
just verify-stats-search-container
just verify
just test-import
```

The focused Rust suite covers the token corpus, multi-batch and coalesced changes, rollback, UTC microsecond boundaries, stable pagination, source-identity and inode replacement with equal acknowledged revisions, missing/unsupported commit payloads, filesystem guards, and writer exclusion. Deterministic pause points test cancellation before and after index commit and prevent a concurrent write from splitting search from hydration. A real native trigger aborts acknowledgement to verify queue rollback after an index commit. The PostgreSQL oracle validates the old result set independently, so deliberate differences remain visible. The container target verifies the backend's pinned release compiler. The importer regression checks that adding search bookkeeping does not change the 41-table data-transfer contract.
