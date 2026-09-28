PRAGMA foreign_keys = ON;

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
 UNIQUE(actor_id,idempotency_key),
 CHECK((status IN ('running','delivering') AND worker_id IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL) OR (status NOT IN ('running','delivering') AND worker_id IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL)),
 CHECK((status='succeeded' AND (delivered_message_id IS NULL OR delivered_message_id>0)) OR (status<>'succeeded' AND delivered_message_id IS NULL))
);
CREATE INDEX operations_claim_order ON runtime_operations(available_at,created_at,id) WHERE status='queued';
CREATE INDEX operations_expiry ON runtime_operations(lease_expires_at,id) WHERE status IN ('running','delivering');
-- Capacity checks must scan pending work, not the retained operation history.
CREATE INDEX operations_pending_actor ON runtime_operations(actor_id) WHERE status IN ('queued','running','delivering');

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

CREATE TABLE runtime_media_cache (
 actor_id INTEGER NOT NULL CHECK(actor_id>0), bot_id INTEGER NOT NULL CHECK(bot_id>0),
 cache_key TEXT NOT NULL CHECK(length(cache_key)=67 AND substr(cache_key,1,3)='v1:' AND substr(cache_key,4) NOT GLOB '*[^0-9a-f]*'),
 revision TEXT NOT NULL,
 media TEXT NOT NULL CHECK(json_valid(media) AND json_type(media)='array' AND json_array_length(media) BETWEEN 1 AND 100 AND length(CAST(media AS BLOB))<=512000),
 expires_at INTEGER NOT NULL,
 created_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
 PRIMARY KEY(actor_id,bot_id,cache_key)
);
CREATE INDEX media_cache_expiry_idx ON runtime_media_cache(expires_at);
