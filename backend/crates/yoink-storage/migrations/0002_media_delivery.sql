ALTER TABLE yoink_runtime.operations
    ADD COLUMN available_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ADD COLUMN retry_delay_secs INTEGER NOT NULL DEFAULT 0 CHECK (retry_delay_secs BETWEEN 0 AND 300),
    ADD COLUMN delivered_message_id BIGINT,
    ADD COLUMN delivery_started_at TIMESTAMPTZ;

ALTER TABLE yoink_runtime.operations
    DROP CONSTRAINT operations_status_check,
    ADD CONSTRAINT operations_status_check CHECK (
        status IN ('queued', 'running', 'delivering', 'delivery_unknown', 'succeeded', 'failed', 'cancelled')
    ),
    DROP CONSTRAINT operations_lease_consistent,
    ADD CONSTRAINT operations_lease_consistent CHECK (
        (status IN ('running', 'delivering') AND worker_id IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR
        (status NOT IN ('running', 'delivering') AND worker_id IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL)
    ),
    ADD CONSTRAINT operations_delivery_consistent CHECK (
        (status = 'succeeded' AND (delivered_message_id IS NULL OR delivered_message_id > 0))
        OR (status <> 'succeeded' AND delivered_message_id IS NULL)
    );
