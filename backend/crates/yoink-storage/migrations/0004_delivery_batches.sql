ALTER TABLE yoink_runtime.operations
    DROP CONSTRAINT operations_status_check,
    ADD CONSTRAINT operations_status_check CHECK (
        status IN ('queued', 'running', 'delivering', 'delivery_unknown', 'delivery_partial', 'succeeded', 'failed', 'cancelled')
    ),
    ADD COLUMN planned_artifact_count INTEGER CHECK (planned_artifact_count BETWEEN 1 AND 100),
    ADD COLUMN delivered_artifact_count INTEGER NOT NULL DEFAULT 0 CHECK (delivered_artifact_count BETWEEN 0 AND 100);

CREATE TABLE yoink_runtime.delivery_units (
    operation_id UUID NOT NULL REFERENCES yoink_runtime.operations(id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 99),
    artifact_count INTEGER NOT NULL CHECK (artifact_count BETWEEN 1 AND 10),
    status TEXT NOT NULL CHECK (status IN ('sending', 'sent', 'rejected', 'unknown')),
    message_ids INTEGER[],
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (operation_id, attempt, ordinal),
    CHECK ((status = 'sent' AND message_ids IS NOT NULL AND cardinality(message_ids) = artifact_count)
        OR (status <> 'sent' AND message_ids IS NULL))
);
CREATE INDEX delivery_units_attempt_status ON yoink_runtime.delivery_units(operation_id, attempt, ordinal);
