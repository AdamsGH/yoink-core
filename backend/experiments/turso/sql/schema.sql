PRAGMA foreign_keys = ON;
CREATE TABLE actors (
    id INTEGER PRIMARY KEY
);
CREATE TABLE requests (
    actor_id INTEGER NOT NULL REFERENCES actors(id),
    request_key TEXT NOT NULL,
    PRIMARY KEY (actor_id, request_key)
);
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    actor_id INTEGER NOT NULL,
    request_key TEXT NOT NULL,
    payload TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'leased', 'sending', 'sent', 'delivery_unknown')),
    lease_generation INTEGER NOT NULL DEFAULT 0 CHECK (lease_generation >= 0),
    lease_token TEXT,
    lease_expires_at INTEGER,
    FOREIGN KEY (actor_id, request_key) REFERENCES requests(actor_id, request_key),
    UNIQUE (actor_id, request_key),
    CHECK ((state IN ('leased', 'sending') AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (state NOT IN ('leased', 'sending') AND lease_token IS NULL AND lease_expires_at IS NULL))
);
CREATE TABLE audit_rows (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id)
);
