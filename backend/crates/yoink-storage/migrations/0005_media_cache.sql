ALTER TABLE yoink_runtime.operations
    DROP CONSTRAINT operations_max_attempts_check,
    ADD CONSTRAINT operations_max_attempts_check CHECK (max_attempts BETWEEN 1 AND 101),
    ADD COLUMN cache_bypassed BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN cache_key VARCHAR(67) CHECK (cache_key IS NULL OR cache_key ~ '^v1:[0-9a-f]{64}$');

CREATE TABLE yoink_runtime.media_cache (
    actor_id BIGINT NOT NULL CHECK (actor_id > 0),
    bot_id BIGINT NOT NULL CHECK (bot_id > 0),
    cache_key VARCHAR(67) NOT NULL CHECK (cache_key ~ '^v1:[0-9a-f]{64}$'),
    revision UUID NOT NULL,
    media JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (actor_id, bot_id, cache_key),
    CHECK (jsonb_typeof(media) = 'array' AND jsonb_array_length(media) BETWEEN 1 AND 100),
    CHECK (octet_length(media::text) <= 512000)
);

CREATE INDEX media_cache_expiry_idx ON yoink_runtime.media_cache (expires_at);
