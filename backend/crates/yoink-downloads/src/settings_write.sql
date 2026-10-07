-- One statement preserves unrelated fields and rechecks global blocking at the write boundary.
INSERT INTO dl_user_settings
 (user_id, quality, codec, container, audio_codec, send_as_file, gallery_zip, subs_lang, subs_auto, nsfw_blur, subs_enabled, subs_always_ask, split_size, keyboard, mediainfo, use_pool_cookies, created_at, updated_at)
SELECT id,
 CASE WHEN ?2 = 'quality' THEN ?3 ELSE 'best' END,
 CASE WHEN ?2 = 'codec' THEN ?3 ELSE 'avc1' END,
 CASE WHEN ?2 = 'container' THEN ?3 ELSE 'mp4' END,
 CASE WHEN ?2 = 'audio_codec' THEN ?3 ELSE 'best' END,
 CASE WHEN ?2 = 'send_as_file' AND ?3 = 'true' THEN 1 ELSE 0 END,
 CASE WHEN ?2 = 'gallery_zip' AND ?3 = 'true' THEN 1 ELSE 0 END,
 CASE WHEN ?2 = 'subs_lang' THEN ?3 ELSE 'en' END,
 CASE WHEN ?2 = 'subs_auto' AND ?3 = 'true' THEN 1 ELSE 0 END,
 CASE WHEN ?2 = 'nsfw_blur' AND ?3 = 'false' THEN 0 ELSE 1 END,
 CASE WHEN ?2 = 'subs_enabled' THEN CASE WHEN ?3 = 'true' THEN 1 ELSE 0 END ELSE 0 END,
 CASE WHEN ?2 = 'subs_always_ask' THEN CASE WHEN ?3 = 'true' THEN 1 ELSE 0 END ELSE 0 END,
 CASE WHEN ?2 = 'split_size' THEN CAST(?3 AS INTEGER) ELSE 2043000000 END,
 CASE WHEN ?2 = 'keyboard' THEN ?3 ELSE '2x3' END,
 CASE WHEN ?2 = 'mediainfo' THEN CASE WHEN ?3 = 'true' THEN 1 ELSE 0 END ELSE 0 END,
 CASE WHEN ?2 = 'use_pool_cookies' THEN CASE WHEN ?3 = 'true' THEN 1 ELSE 0 END ELSE 1 END,
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
 nsfw_blur = CASE WHEN ?2 = 'nsfw_blur' THEN excluded.nsfw_blur ELSE dl_user_settings.nsfw_blur END,
 subs_enabled = CASE WHEN ?2 = 'subs_enabled' THEN excluded.subs_enabled ELSE dl_user_settings.subs_enabled END,
 subs_always_ask = CASE WHEN ?2 = 'subs_always_ask' THEN excluded.subs_always_ask ELSE dl_user_settings.subs_always_ask END,
 split_size = CASE WHEN ?2 = 'split_size' THEN excluded.split_size ELSE dl_user_settings.split_size END,
 keyboard = CASE WHEN ?2 = 'keyboard' THEN excluded.keyboard ELSE dl_user_settings.keyboard END,
 mediainfo = CASE WHEN ?2 = 'mediainfo' THEN excluded.mediainfo ELSE dl_user_settings.mediainfo END,
 use_pool_cookies = CASE WHEN ?2 = 'use_pool_cookies' THEN excluded.use_pool_cookies ELSE dl_user_settings.use_pool_cookies END,
 updated_at = max(excluded.updated_at, dl_user_settings.updated_at + 1)
RETURNING user_id;
