-- Module-owned additive DDL; coordinator assigns migration ordering.
-- Private cache includes user, model, prompt, focus, source and research digest.
-- Legacy cache is kept for imported default about/summary results only.
CREATE TABLE insight_native_cache (
 cache_key TEXT PRIMARY KEY NOT NULL,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 result TEXT NOT NULL,
 expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX insight_native_cache_expiry ON insight_native_cache(expires_at);
-- Atomic admission reservations prevent parallel requests exceeding daily quota.
-- Leases expire after the bounded request deadline; failures release reservations.
CREATE TABLE insight_native_reservations (
 id TEXT PRIMARY KEY NOT NULL,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 command TEXT NOT NULL,
 created_at INTEGER NOT NULL,
 expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX insight_native_reservations_user ON insight_native_reservations(user_id,command,created_at);
