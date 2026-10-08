# Native Insight contract repair audit

Authority: executable plugin and frontend at gitlink
`8209ebb76c624228fae289ac5c1e29e9842d3bc8`, read from the populated
`../yoink-core/plugins/yoink-insight` checkout (the isolated gitlink directory is
empty). Native baseline: `611ff142`. Historical module README/test totals are not
acceptance criteria. No module SQL, shared files or worker_content.rs changed.
The coordinator's migration-24 registration in `057f25b4` remains independent.

## Candidate dispositions

1. **Settings — confirmed and repaired.** `api/router.py` get/update settings
   allows any TLDR-granted user to configure a GitHub read token, but reserves
   model/search controls for gateway grants. Language PATCH requires summary or
   TLDR access; inactive command prompt updates are silently dropped. Native now
   follows those rules, masks inactive model/search/token indicators, supports
   en/ru/uk/de/fr/es/zh/ja, honors imported settings and configured default language,
   and lets owners select models outside the allowlist. Tests include
   `byok_settings_languages_prompts_masking_and_alias_partial_merge`, existing
   explicit-null model reset checks, and owner/nonowner model checks.

2. **Aliases/parsing — confirmed and repaired.** `router.py` alias CRUD merges
   omitted/nullable PATCH fields with the owned row; explicit blank CSV clears,
   and normalized empty CSV persists as NULL. Native enforces TLDR list/create
   access, owned edit/delete without a continuing TLDR grant, 20 rows, conflict
   status 409, Unicode alphanumeric/hyphen keywords, and bracket fnmatch patterns.
   `services/tldr.py` and `commands/tldr.py` bind domains only without explicit
   alias or question. Native now suppresses domain binding for focus requests.
   `commands/summary.py::_extract_youtube_url` searches the entire text; native
   does too and avoids requiring alias-list permission for summary-only users.
   Existing ownership/conflict tests remain, with additional partial-merge,
   Unicode/bracket/UTF-16 and summary-only Telegram checks. Builtin conflict status
   is deliberately 409 per this repair contract (legacy validator used 400).

3. **Access — confirmed and repaired.** `router.py` lookup strips @, requires a
   nonempty query, returns at most 20, uses exact numeric ID or username matching;
   list orders ascending granted_at. POST requires an existing user and returns
   201; PATCH changes only the existing legacy row's language, never grant
   metadata or permissions; missing DELETE returns 404. Unified summary grant
   writes/revokes remain atomic with the compatibility view, not weakened to
   mimic the legacy router's incomplete grant writes. Callback regression checks
   missing targets and PATCH metadata preservation.

4. **Provider readiness/endpoints — confirmed and repaired.**
   `services/byok.py::_verify_openrouter_key` authenticates /key before the public
   /models catalogue. Perplexity uses tiny sonar chat and five sonar fallback
   models, not an unsupported catalogue. Native readiness is provider-aware even
   for a configured/custom provider base. Gemini BYOK uses OpenAI-compatible
   /v1beta/openai, while operator Gemini summary/about keep native Gemini shape.
   Anthropic root, /v1 and full /v1/messages bases work for completion/stream and
   catalogue. Seven-provider contract regression plus actual loopback OpenRouter
   invalid-key/Perplexity ping and all three wire shapes are covered. Revision,
   toggle, ban and post-I/O credential checks remain; no BYOK fallback or inherited
   proxy is permitted. Explicit operator OpenRouter proxy builder added.
   Real provider availability and production egress policy are not locally proven.

5. **Route/search — confirmed and repaired against executable behavior.**
   `services/tldr.py::resolve_tldr_route` calls `InsightAccessService.is_allowed`;
   `services/access.py` actually checks explicit **summary**, not the tldr feature
   claimed by the resolver docstring. Owner/explicit nonexpired summary therefore
   takes gateway precedence; an explicit tldr grant alone does not override ready
   BYOK. The regression expressly distinguishes those cases. Inherited research
   is suppressed for BYOK and OpenRouter-only paths. Research failures degrade to
   source-only instructions without fabricated citations; cancellation remains
   an error. Absent allowlists bootstrap `[Config.model]`, matching router.py's
   executable config fallback, not a persisted-default-model reinterpretation.

6. **Secrets/presence — confirmed and repaired without raw-key responses.**
   Redacted config/BYOK responses remain. Existing config *_api_key_set booleans
   remain the safe frontend indicators; no raw-key public field was added.
   Omitted credentials preserve, explicit empty clears. BYOK clear also removes
   readiness and grants; an explicitly empty probe never uses the stored key.
   OpenRouter-only config can omit the unused gateway URL. One original
   unchanged-key fixture now omits api_key instead of sending whitespace; its
   assertions were preserved, and a separate empty-clear regression added.
   Frontend must implement this presence contract before integration.

7. **Quota/cache — repaired, with imported cache quarantine after independent review.**
   Executable `storage/repos.py::count_today` counts all successful commands,
   excluding cached/errors; count_today_command is exact `tldr`. Summary's private
   native cache lookup precedes its quota; TLDR admission precedes native cache.
   Legacy `commands/tldr.py` uses no-focus tldr[:alias]:v3 keys, but writes BYOK,
   private GitHub and custom-alias results into that shared table too (459–462).
   `commands/_runner.py` likewise caches prompt_override results (183–185).
   The imported table has no producer identity, prompt, credential or route
   provenance. Reader-side default settings cannot establish producer safety.
   **All imported insight_summary_cache entries are therefore quarantined from
   serving**, for summary, about and TLDR. Rows and results remain unchanged for
   migration/audit. No expiry, builtin-alias or current-reader heuristic bypasses
   this rule; there is no trustworthy producer-provenance column in these entries.
   Earlier claims in this audit that shared imports were safe to reuse were wrong.
   The historical native.sql cache comment is stale; SQL is deliberately unchanged
   because its registered migration checksum must remain stable.
   Native replay keys include user, URL, language, instruction, credential/route
   and search configuration. Verified private native repeats still work offline;
   focus requests are neither read from nor written to caches. Transactional
   admissions/counting and cache+success accounting prevent partial success.
   Normal failure/cancel releases synchronously; dropped futures release via a
   runtime cleanup task, with a bounded lease as a crash backstop. Tests cover
   concurrent limits, aborted tasks, native cache/quota ordering, missing-provider
   imported-cache rejection, cross-user private GitHub/BYOK/custom-alias and
   prompt-override import rejection, preserved imported rows, focus repeats,
   offline native replay and atomic accounting. Imported-cache tests now assert
   quarantine, intentionally superseding their previous unsafe reuse assertions.

8. **Stats/Telegram — confirmed and repaired.** `router.py` sorts aliases by
   count, uses now-minus-30-days rather than midnight, and computes zero-duration
   YouTube fallback as chars / 5.5 / 150. Native now follows those imported metrics
   (positive video seconds still /60, web chars /1000). Telegram chunks use at
   most 4000 UTF-16 code units, not scalar count. Deterministic imported-row and
   supplementary-plane chunk regressions cover the changes.

9. **OAuth — confirmed and repaired.** `router.py` write-connect is available to
   authenticated users when its separate app is configured, without requiring a
   new gh_write grant. Native write OAuth now requires a currently nonblocked
   identity; actual write operations retain InboxAdapter's gh_write gate. Read
   flow still requires TLDR. Separate tokens, scope checks, generation locks,
   expiry, post-username reauthorization, cancellation/no resurrection and
   shutdown joining are preserved. Independent review also found a persistence
   gap after the pre-DB cancellation/expiry check. Final acquisition now races
   cancellation/expiry without holding the generation mutex; after acquisition,
   the current generation is locked and authority is checked inside a transaction.
   Cancellation/expiry are rechecked after acquisition, immediately before the
   write, and before commit; stale attempts roll back. Disconnect releases its
   generation mutex before DB acquisition to avoid lock-order inversion. A
   deterministic held-DB test covers shutdown, cancel and expiry for read and
   write tokens, asserting no persisted token or success. Missing app maps to 501. Trusted typed
   OAuthOptions supplies bounded time budgets and endpoint injection; the real
   loopback read/write/username/revocation fixtures use this public constructor.
   Full runtime-process disconnect/menu/lifecycle acceptance remains coordinator
   work, not established by library tests.

## Public coordinator hooks and remaining integration

See INTEGRATION.md's contract-repair section for exact constructors and signatures:
committed access-change callback, Config.default_lang, OAuthOptions/with_options,
DeviceOAuth cancel/shutdown, and explicit operator proxy builder. Bind the callback
before cloning; its synchronous, nonblocking signal survives response failure and
must feed `IdentityStore::notify_committed_access_change()` (public in main fb5fd7d8).
Shared runtime, HTTP mounting/disconnect, frontend presence handling, Telegram
menu/delivery, schema integration and actual-process end-to-end fixtures remain
coordinator-owned. Confinement of the content worker remains its worker owner's
responsibility. No live provider checks or shared image builds were performed.

This report does **not** declare full module migration acceptance.
