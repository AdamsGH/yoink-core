-- Candidate is ONLY the outgoing scoped cut's linked quality UUID, captured
-- before its save. Reclaim it after cancellation/replacement removes the last
-- replayable cut reference, unless a current quality draft still requires it.
DELETE FROM runtime_quality_media_intents
WHERE session_id=?1
AND NOT EXISTS (
    SELECT 1 FROM runtime_quality_sessions
    WHERE id=runtime_quality_media_intents.session_id
)
AND NOT EXISTS (
    SELECT 1 FROM runtime_cut_sessions
    WHERE linked_quality_id=runtime_quality_media_intents.session_id
    AND stage<>'cancelled'
);
