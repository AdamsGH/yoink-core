INSERT INTO runtime_quality_media_intents(session_id,intent) VALUES(?1,?2) ON CONFLICT(session_id) DO NOTHING;
