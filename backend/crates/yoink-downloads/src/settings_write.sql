-- One statement preserves unrelated fields and rechecks global blocking at the write boundary.
INSERT INTO dl_user_settings
 (user_id, quality, codec, container, audio_codec, send_as_file, gallery_zip, subs_lang, subs_auto, created_at, updated_at)
SELECT id,
 CASE WHEN ?2 = 'quality' THEN ?3 ELSE 'best' END,
 CASE WHEN ?2 = 'codec' THEN ?3 ELSE 'avc1' END,
 CASE WHEN ?2 = 'container' THEN ?3 ELSE 'mp4' END,
 CASE WHEN ?2 = 'audio_codec' THEN ?3 ELSE 'best' END,
 CASE WHEN ?2 = 'send_as_file' AND ?3 = 'true' THEN 1 ELSE 0 END,
 CASE WHEN ?2 = 'gallery_zip' AND ?3 = 'true' THEN 1 ELSE 0 END,
 CASE WHEN ?2 = 'subs_lang' THEN ?3 ELSE 'en' END,
 CASE WHEN ?2 = 'subs_auto' AND ?3 = 'true' THEN 1 ELSE 0 END,
 ?4, ?4
FROM users
WHERE id = ?1 AND (role IN ('user', 'moderator', 'admin', 'owner') OR (?6 = 1 AND role = 'restricted'))
 AND (ban_until IS NULL OR ban_until <= ?4)
 AND (?5 IS NULL OR COALESCE((SELECT updated_at FROM dl_user_settings WHERE user_id = ?1), 0) = ?5)
ON CONFLICT(user_id) DO UPDATE SET
 quality = CASE WHEN ?2 = 'quality' THEN excluded.quality ELSE dl_user_settings.quality END,
 codec = CASE WHEN ?2 = 'codec' THEN excluded.codec ELSE dl_user_settings.codec END,
 container = CASE WHEN ?2 = 'container' THEN excluded.container ELSE dl_user_settings.container END,
 audio_codec = CASE WHEN ?2 = 'audio_codec' THEN excluded.audio_codec ELSE dl_user_settings.audio_codec END,
 send_as_file = CASE WHEN ?2 = 'send_as_file' THEN excluded.send_as_file ELSE dl_user_settings.send_as_file END,
 gallery_zip = CASE WHEN ?2 = 'gallery_zip' THEN excluded.gallery_zip ELSE dl_user_settings.gallery_zip END,
 subs_lang = CASE WHEN ?2 = 'subs_lang' THEN excluded.subs_lang ELSE dl_user_settings.subs_lang END,
 subs_auto = CASE WHEN ?2 = 'subs_auto' THEN excluded.subs_auto ELSE dl_user_settings.subs_auto END,
 updated_at = max(excluded.updated_at, dl_user_settings.updated_at + 1)
RETURNING user_id;
