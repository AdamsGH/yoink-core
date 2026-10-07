-- Prepared is durable application intent, not an external attempt.
-- Independent of media claims. A sending row is already beyond the retry boundary.
CREATE TABLE runtime_text_effects (
    token TEXT PRIMARY KEY NOT NULL CHECK(length(token) BETWEEN 1 AND 256),
    identity TEXT CHECK(identity IS NULL OR json_valid(identity)),
    state TEXT NOT NULL CHECK(state IN ('prepared','sending','unknown','confirmed','sealed')),
    receipt INTEGER CHECK(receipt IS NULL OR receipt > 0),
    bound INTEGER NOT NULL DEFAULT 0 CHECK(bound IN (0,1)),
    CHECK((state='sealed' AND identity IS NULL AND receipt IS NULL) OR
          (state IN ('prepared','sending','unknown') AND identity IS NOT NULL AND receipt IS NULL) OR
          (state='confirmed' AND identity IS NOT NULL AND receipt IS NOT NULL))
) STRICT;
CREATE INDEX runtime_text_effects_recovery ON runtime_text_effects(state,bound);
