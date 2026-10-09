# Native Stats

Stats owns ingestion, analytics, command rendering and the derived Tantivy writer. SQLite business tables remain authoritative. No Stats operation registers users, grants permissions or changes core roles.

## Runtime integration

Apply `sql/native.sql` through shared migrations before opening Stats. It adds runtime ordering/revision state and indexes, not replacement business tables. Canonical subjects are shared across bots. Newer Telegram date/edit_date wins; equal dates use update_id only for the same bot. Cross-bot equal dates are first-accepted-wins. Names retain changed-profile history at monotonic accepted observation times; persistent imported freshness floors prevent historical replay from replacing a newer imported profile.

Successful revision-protected profile reconciliation also advances a separate per-user lookup-start Telegram-second floor, for explicit name refreshes and roster-driven names. Delayed observations strictly older than either floor cannot replace the profile; equal-second observations remain eligible under canonical ordering. Missing profiles, failed lookups and revision conflicts never advance this floor. The imported floor and history remain unchanged.

Open one `Stats` with the shared database, fresh `IdentityStore`, a trusted `MembershipProvider`, optional native user-session client and a private index directory. Feed original raw group updates to `observe_raw_update`. Private and disabled groups and bot-command messages are excluded. Message edits, names, membership/service events and per-user reactions are transactionally replay-safe. Bot API does not report ordinary group-message deletions. Native database deletion triggers create search tombstones when another authorized native operation deletes a row.

Implement both provider methods with fresh Bot API results. `lookup` returns membership and administrator state. `lookup_name(Some(chat), user)` uses getChatMember; `lookup_name(None, user)` is the getChat fallback. Return None for unavailable, unknown, deleted or inaccessible profiles. Name lookup is timeout-bounded; missing metadata never erases known names. No registration policy is called for discovered IDs.

Schedule and drain `refresh_usernames(schedule.lookup_timeout)` using `UsernameRefreshSchedule`. Defaults match the legacy JobSpec: first run after 60 seconds, then every 3600 seconds. Parent configuration supplies the interval from `STATS_REFRESH_INTERVAL`. The product does not spawn an unowned background task. User-session roster synchronization pages 200 members at a time, limits the scan to 500 pages, deduplicates overlapping pages, and atomically reconciles members/admins/names after complete pagination and protected reauthorization. Revision fences preserve subjects observed during lookup/scanning and reject stale reconciliation from overlapping scans. Only successful roster reconciliation advances a separate scan-start second-resolution freshness floor; same-second live events remain eligible.

Mount `http::router(stats)` below `/stats`, after shared Telegram WebApp/JWT authentication. Bridge the verified principal to `http::AuthenticatedUser`; never derive this extension from an unverified header or bearer claim. Analytics allow admin+, assigned moderators, or fresh Telegram members. Members allow admin+ or fresh Telegram chat administrators. Reactions and roster synchronization require admin+. HTTP/command reads recheck native identity and local assignment state under the protected database guard after membership lookup. The router intentionally does not implement a second authentication mechanism.

Register `/stats` with the shared command dispatcher. Pass user ID, chat ID, already split arguments and native i18n Language to `Stats::command`. It returns independently valid escaped HTML chunks, not JSON or Telegram transport objects. Private `user` commands aggregate only individually authorized enabled groups.

`Stats::search` authorizes before accessing the shared native search implementation. Schedule `refresh` as required by the runtime. `recover_search` rebuilds from SQLite; `recover_search_at` replaces an unreadable index using a fresh private directory without deleting operator data. Stop admission and drain ingestion/name-refresh jobs before `shutdown`, which waits for active search work, flushes derived state and releases the writer lock.

## HTTP and ownership boundaries

Implemented routes: groups, members/sync, overview, top-users, activity-by-hour/day/week/month, history, message-types, words, user-stats, ecdf, title-history, mention-stats, daily-activity, member-events, avg-message-length, response-time, media-trend, top-reactions, members and chat-admins. They retain legacy frontend field names, UTC buckets, name fallback and null behavior. Commands preserve their separate words, mention-direction and quote-eligibility contracts, all-history week defaults, and history windows relative to a supplied end.

Message aggregates use indexed 256-key windows, SQL date/user/type filters and required-column reads. Tokenization and reductions run outside the database guard; streak/correlation retain per-date state, not messages. Exact word counts retain distinct vocabulary. Reply medians retain at most 86400 second bins per user, with extrema preserving formatted cross-bin medians. Filtered member hydration releases the guard between bounded batches. A changed source revision returns recoverable `stats_unavailable` (HTTP 503) rather than mixing snapshots. Ordinary auxiliary SQLite reducers and native search rebuilds still perform database work under their shared guards. Production-scale throughput has not been benchmarked.

Parent owns server/Telegram/HTTP composition, shared migrations, import/background-job endpoints and shared `/threads` routes. This crate neither accepts import paths nor creates a second import worker. The source compatibility and outstanding composition contracts are recorded in the run report.

## Validation

From `backend`: `just check-crate yoink-stats`, `just lint-crate yoink-stats`, `just test-crate yoink-stats`. Tests use temporary SQLite databases and real Tantivy directories, with an ephemeral loopback user-session server for paginated roster/failure cases. No production credentials or services are used.
