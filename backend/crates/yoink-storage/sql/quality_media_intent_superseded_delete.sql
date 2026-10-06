-- Remove only the original belonging to the quality UUID replaced in this scope.
-- Same-UUID revision writes must retain it. A surviving cut handoff still uses
-- its linked UUID in DownloadService::submit_quality_media, including replay.
DELETE FROM runtime_quality_media_intents
WHERE session_id IN (
    SELECT id FROM runtime_quality_sessions
    WHERE actor_id=?1 AND bot_id=?2 AND chat_id=?3 AND id<>?4
)
AND NOT EXISTS (
    SELECT 1 FROM runtime_cut_sessions
    WHERE linked_quality_id=runtime_quality_media_intents.session_id
    AND stage<>'cancelled'
);
