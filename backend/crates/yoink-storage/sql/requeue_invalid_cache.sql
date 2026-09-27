WITH locked AS MATERIALIZED (
    SELECT op.*
    FROM yoink_runtime.operations op
    WHERE op.id = $1 AND op.attempt = $2 AND op.lease_token = $3 AND op.worker_id = $4
      AND op.status = 'delivering' AND op.lease_expires_at > clock_timestamp()
      AND NOT op.cancel_requested AND NOT op.cache_bypassed
      AND op.delivered_artifact_count = 0
      AND op.request->>'actor_id' = op.actor_id::text
      AND op.request->'target'->>'bot_id' = $7::bigint::text
      AND op.cache_key = $5
      AND NOT EXISTS (
          SELECT 1 FROM yoink_runtime.delivery_units du
          WHERE du.operation_id = op.id AND du.attempt = op.attempt
            AND du.status = 'sent'
      )
    FOR UPDATE
), rejected AS (
    UPDATE yoink_runtime.delivery_units du
    SET status = 'rejected', updated_at = clock_timestamp()
    FROM locked op
    WHERE du.operation_id = op.id AND du.attempt = op.attempt
      AND du.ordinal = 0 AND du.status = 'sending'
      AND NOT EXISTS (
          SELECT 1 FROM yoink_runtime.delivery_units other
          WHERE other.operation_id = op.id AND other.attempt = op.attempt
            AND other.status IN ('sending', 'unknown') AND other.ordinal <> 0
      )
    RETURNING du.operation_id, du.attempt
), requeued AS (
    UPDATE yoink_runtime.operations op
    SET status = 'queued', max_attempts = op.max_attempts + 1,
        available_at = clock_timestamp(), worker_id = NULL, lease_token = NULL,
        lease_expires_at = NULL, cache_bypassed = TRUE,
        planned_artifact_count = NULL, delivered_artifact_count = 0,
        delivered_message_id = NULL, delivery_started_at = NULL,
        progress_percent = NULL, progress_stage = NULL, updated_at = clock_timestamp()
    FROM locked, rejected
    WHERE op.id = rejected.operation_id AND op.id = locked.id
      AND op.attempt = rejected.attempt
    RETURNING op.actor_id, (op.request->'target'->>'bot_id')::bigint AS bot_id
), evicted AS (
    DELETE FROM yoink_runtime.media_cache mc
    USING requeued r
    WHERE mc.actor_id = r.actor_id AND mc.bot_id = r.bot_id
      AND mc.cache_key = $5 AND mc.revision = $6
    RETURNING 1
)
SELECT EXISTS (SELECT 1 FROM requeued) AS requeued;
