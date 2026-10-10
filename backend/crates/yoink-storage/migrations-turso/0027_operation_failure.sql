ALTER TABLE runtime_operations ADD COLUMN failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
 'unsupported_source','source_unavailable','rate_limited','invalid_media','processing_failed','resource_limit',
 'cookie_required','geo_blocked','age_restricted','private_content','live_stream','no_media','media_timeout',
 'network_denied','content_unavailable','source_blocked'
));
-- Canonicalize ADD COLUMN DDL so native VACUUM snapshots retain the exact fingerprint.
ALTER TABLE runtime_operations RENAME COLUMN failure_code TO failure_code_v27;
ALTER TABLE runtime_operations RENAME COLUMN failure_code_v27 TO failure_code;
