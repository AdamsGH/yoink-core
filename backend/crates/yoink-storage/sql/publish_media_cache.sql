WITH eligible AS MATERIALIZED (
    SELECT op.id, op.actor_id, (op.request->'target'->>'bot_id')::bigint AS bot_id
    FROM yoink_runtime.operations op
    WHERE op.id = $1 AND op.attempt = $2 AND op.status = 'succeeded'
      AND op.actor_id = $3
      AND op.request->>'actor_id' = $3::text
      AND op.request->'target'->>'bot_id' = $4::bigint::text
      AND op.planned_artifact_count = $6
      AND op.delivered_artifact_count = $6
      AND NOT op.cancel_requested
      AND $6 BETWEEN 1 AND 100
      AND NOT EXISTS (
          SELECT 1 FROM yoink_runtime.delivery_units du
          WHERE du.operation_id = op.id AND du.attempt = op.attempt AND du.status <> 'sent'
      )
      AND COALESCE((
          SELECT SUM(du.artifact_count)
          FROM yoink_runtime.delivery_units du
          WHERE du.operation_id = op.id AND du.attempt = op.attempt AND du.status = 'sent'
      ), 0) = $6
      AND op.request = $10::jsonb
      AND op.cache_key = $5
), expired AS MATERIALIZED (
    SELECT mc.actor_id, mc.bot_id, mc.cache_key
    FROM yoink_runtime.media_cache mc
    WHERE mc.expires_at <= clock_timestamp()
      AND EXISTS (SELECT 1 FROM eligible)
      -- The current key is upserted below, never delete and update the same
      -- row in two data-modifying CTEs of one statement.
      AND NOT (mc.actor_id = $3 AND mc.bot_id = $4::bigint AND mc.cache_key = $5)
    ORDER BY mc.expires_at
    LIMIT 32
    FOR UPDATE SKIP LOCKED
), pruned AS (
    DELETE FROM yoink_runtime.media_cache mc USING expired e
    WHERE mc.actor_id = e.actor_id AND mc.bot_id = e.bot_id AND mc.cache_key = e.cache_key
    RETURNING 1
), stored AS (
    INSERT INTO yoink_runtime.media_cache(actor_id, bot_id, cache_key, revision, media, expires_at)
    SELECT actor_id, bot_id, $5, $7, $8::jsonb,
           clock_timestamp() + ($9::bigint * INTERVAL '1 second')
    FROM eligible
    ON CONFLICT (actor_id, bot_id, cache_key) DO UPDATE
    SET revision = EXCLUDED.revision, media = EXCLUDED.media,
        expires_at = EXCLUDED.expires_at, created_at = clock_timestamp()
    RETURNING 1
)
SELECT EXISTS (SELECT 1 FROM stored) AS published;
