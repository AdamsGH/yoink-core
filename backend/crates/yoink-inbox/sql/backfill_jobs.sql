-- Run in the same coordinator-owned migration transaction after durable_jobs.sql.
-- Imported metadata is not trusted for authorization: worker rechecks current grants.
INSERT INTO inbox_jobs(user_id,item_id,kind,available_at,created_at)
SELECT user_id,id,'enrich',modified_at,modified_at FROM inbox_items
WHERE status='pending' AND archived_at IS NULL
ON CONFLICT DO NOTHING;
INSERT INTO inbox_jobs(user_id,item_id,kind,available_at,created_at)
SELECT user_id,id,'classify',modified_at,modified_at FROM inbox_items
WHERE status='enriched' AND archived_at IS NULL
ON CONFLICT DO NOTHING;
