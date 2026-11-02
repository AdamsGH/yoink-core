# Stats

Stats records eligible group activity and serves analytics through Telegram commands and the Mini App. Native rows remain authoritative. A separate Tantivy index supports message search.

Private messages, disabled groups, and bot-command messages do not enter normal group ingestion. The Bot API does not report ordinary group-message deletions.

## Commands

Run `/stats` in a group to see the supported commands.

```text
/stats counts -n 10
/stats types
/stats words -n 20
/stats history --days 7
/stats user
```

Private chats support only `user` summaries across individually authorized enabled groups.

The parser also supports date bounds, user/type filters, and `-q` or `--lquery` where the selected command accepts them. See the [command implementation](../../backend/crates/yoink-stats/src/commands.rs) for the complete grammar.

## Access

Analytics require current administrative authority, a local moderator assignment, or verified Telegram membership, depending on the operation.

Member administration and synchronization have stricter checks. A dashboard route or cached membership result cannot replace current authorization.

## Configuration

Native settings use the `YOINK_RUST_` prefix. Relevant controls include:

- `YOINK_RUST_STATS_ENABLED`
- `YOINK_RUST_STATS_INDEX_DIR`
- `YOINK_RUST_STATS_REFRESH_INTERVAL_SECS`
- `YOINK_RUST_STATS_IMPORT_ROOT`

The supplied Compose stack enables Stats and places its index and import directory under private native state.

Username refresh starts after 60 seconds by default and repeats every 3600 seconds. Failed lookups do not erase known names.

## Search and imports

[Search](stats-search.md) uses an explicit tokenizer contract. It is not identical to PostgreSQL full-text search.

Imports run through the native job lifecycle with bounded files and record counts. Rebuilding the derived index does not replace application rows.

The [product configuration reference](../architecture/products.md) lists limits and lifecycle behavior. No production-scale throughput claim is made.
