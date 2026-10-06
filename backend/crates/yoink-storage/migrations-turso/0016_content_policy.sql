-- ADD COLUMN makes pinned Turso store DDL that VACUUM INTO reformats, breaking
-- strict snapshot fingerprints. Rebuild with canonical CREATE (no ALTER/RENAME).
-- Preserve FK children before dropping the parent: foreign_keys stays enabled,
-- and parent DROP must not cascade-delete confirmed batches or progress notices.
-- Preserve rowids as well as every durable column inside the migration transaction.
CREATE TABLE runtime_operations_v15 AS SELECT rowid AS migration_rowid, * FROM runtime_operations;
CREATE TABLE runtime_delivery_units_v15 AS SELECT rowid AS migration_rowid, * FROM runtime_delivery_units;
CREATE TABLE runtime_progress_notices_v15 AS SELECT rowid AS migration_rowid, * FROM runtime_progress_notices;
DROP TABLE runtime_delivery_units;
DROP TABLE runtime_progress_notices;
DROP TABLE runtime_operations;

CREATE TABLE runtime_operations (
 id TEXT PRIMARY KEY NOT NULL, actor_id INTEGER NOT NULL CHECK(actor_id>0),
 idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
 request TEXT NOT NULL CHECK(json_valid(request) AND json_type(request)='object'),
 status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','running','delivering','delivery_unknown','delivery_partial','succeeded','failed','cancelled')),
 attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt>=0), max_attempts INTEGER NOT NULL CHECK(max_attempts BETWEEN 1 AND 101),
 cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
 worker_id TEXT, lease_token TEXT, lease_expires_at INTEGER,
 created_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
 updated_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
 available_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
 retry_delay_secs INTEGER NOT NULL DEFAULT 0 CHECK(retry_delay_secs BETWEEN 0 AND 300),
 delivered_message_id INTEGER, delivery_started_at INTEGER,
 progress_percent INTEGER CHECK(progress_percent BETWEEN 0 AND 100),
 progress_stage TEXT CHECK(progress_stage IS NULL OR progress_stage IN ('fetching','processing','writing_artifact')),
 planned_artifact_count INTEGER CHECK(planned_artifact_count BETWEEN 1 AND 100),
 delivered_artifact_count INTEGER NOT NULL DEFAULT 0 CHECK(delivered_artifact_count BETWEEN 0 AND 100),
 cache_bypassed INTEGER NOT NULL DEFAULT 0 CHECK(cache_bypassed IN (0,1)),
 cache_key TEXT CHECK(cache_key IS NULL OR (length(cache_key)=67 AND substr(cache_key,1,3)='v1:' AND substr(cache_key,4) NOT GLOB '*[^0-9a-f]*')),
 content_failure TEXT CHECK(content_failure IS NULL OR content_failure IN ('nsfw_blocked','nsfw_policy_unavailable')),
 UNIQUE(actor_id,idempotency_key),
 CHECK((status IN ('running','delivering') AND worker_id IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL) OR (status NOT IN ('running','delivering') AND worker_id IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL)),
 CHECK((status='succeeded' AND (delivered_message_id IS NULL OR delivered_message_id>0)) OR (status<>'succeeded' AND delivered_message_id IS NULL))
);
CREATE INDEX operations_claim_order ON runtime_operations(available_at,created_at,id) WHERE status='queued';
CREATE INDEX operations_expiry ON runtime_operations(lease_expires_at,id) WHERE status IN ('running','delivering');
-- Capacity checks must scan pending work, not the retained operation history.
CREATE INDEX operations_pending_actor ON runtime_operations(actor_id) WHERE status IN ('queued','running','delivering');

CREATE INDEX operations_actor_bot_source_url ON runtime_operations(
 actor_id,
 json_extract(request,'$.target.bot_id'),
 json_extract(request,'$.source_url')
);
CREATE INDEX operations_active_actor ON runtime_operations(actor_id) WHERE status IN ('running','delivering');
INSERT INTO runtime_operations(rowid,id,actor_id,idempotency_key,request,status,attempt,max_attempts,cancel_requested,worker_id,lease_token,lease_expires_at,created_at,updated_at,available_at,retry_delay_secs,delivered_message_id,delivery_started_at,progress_percent,progress_stage,planned_artifact_count,delivered_artifact_count,cache_bypassed,cache_key,content_failure)
SELECT migration_rowid,id,actor_id,idempotency_key,request,status,attempt,max_attempts,cancel_requested,worker_id,lease_token,lease_expires_at,created_at,updated_at,available_at,retry_delay_secs,delivered_message_id,delivery_started_at,progress_percent,progress_stage,planned_artifact_count,delivered_artifact_count,cache_bypassed,cache_key,NULL FROM runtime_operations_v15;

CREATE TABLE runtime_delivery_units (
 operation_id TEXT NOT NULL REFERENCES runtime_operations(id) ON DELETE CASCADE,
 attempt INTEGER NOT NULL CHECK(attempt>0), ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99),
 artifact_count INTEGER NOT NULL CHECK(artifact_count BETWEEN 1 AND 10),
 status TEXT NOT NULL CHECK(status IN ('sending','sent','rejected','unknown')),
 message_ids TEXT CHECK(message_ids IS NULL OR (json_valid(message_ids) AND json_type(message_ids)='array')),
 created_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
 updated_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
 PRIMARY KEY(operation_id,attempt,ordinal),
 CHECK((status='sent' AND message_ids IS NOT NULL AND json_array_length(message_ids)=artifact_count) OR (status<>'sent' AND message_ids IS NULL))
);

INSERT INTO runtime_delivery_units(rowid,operation_id,attempt,ordinal,artifact_count,status,message_ids,created_at,updated_at)
SELECT migration_rowid,operation_id,attempt,ordinal,artifact_count,status,message_ids,created_at,updated_at FROM runtime_delivery_units_v15;

CREATE TABLE runtime_progress_notices (
 operation_id TEXT PRIMARY KEY NOT NULL REFERENCES runtime_operations(id) ON DELETE CASCADE,
 actor_id INTEGER NOT NULL CHECK(actor_id > 0),
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id <> 0),
 message_id INTEGER NOT NULL CHECK(message_id BETWEEN 1 AND 2147483647),
 language TEXT NOT NULL CHECK(language IN ('en','ru')),
 last_text TEXT CHECK(last_text IS NULL OR length(CAST(last_text AS BLOB)) <= 4096),
 finished INTEGER NOT NULL DEFAULT 0 CHECK(finished IN (0,1)),
 next_attempt_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER))
);
CREATE INDEX progress_notices_due ON runtime_progress_notices(bot_id,next_attempt_at,operation_id) WHERE finished=0;
INSERT INTO runtime_progress_notices(rowid,operation_id,actor_id,bot_id,chat_id,message_id,language,last_text,finished,next_attempt_at)
SELECT migration_rowid,operation_id,actor_id,bot_id,chat_id,message_id,language,last_text,finished,next_attempt_at FROM runtime_progress_notices_v15;

DROP TABLE runtime_progress_notices_v15;
DROP TABLE runtime_delivery_units_v15;
DROP TABLE runtime_operations_v15;
