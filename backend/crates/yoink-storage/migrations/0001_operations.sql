CREATE TABLE yoink_runtime.operations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id BIGINT NOT NULL CHECK (actor_id > 0),
    idempotency_key VARCHAR(128) NOT NULL CHECK (idempotency_key <> ''),
    request JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    worker_id VARCHAR(128),
    lease_token UUID,
    lease_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT operations_actor_idempotency_key UNIQUE (actor_id, idempotency_key),
    CONSTRAINT operations_lease_consistent CHECK (
        (status = 'running' AND worker_id IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR
        (status <> 'running' AND worker_id IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL)
    )
);
CREATE INDEX operations_claim_order ON yoink_runtime.operations (created_at, id) WHERE status = 'queued';
