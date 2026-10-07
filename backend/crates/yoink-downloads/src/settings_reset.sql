-- Downloader reset only: credentials, cookies, pool policy, imported extensions and audit history survive.
INSERT INTO dl_user_settings (user_id, created_at, updated_at)
SELECT id, ?4, ?4 FROM users
WHERE id = ?1
 AND (role IN ('user', 'moderator', 'admin', 'owner') OR (?5 = 1 AND role = 'restricted'))
 AND (ban_until IS NULL OR ban_until <= ?4)
 AND COALESCE((SELECT updated_at FROM dl_user_settings WHERE user_id = ?1), 0) BETWEEN 0 AND 9223372036854775806
 AND COALESCE((SELECT updated_at FROM dl_user_settings WHERE user_id = ?1), 0) = ?3
ON CONFLICT(user_id) DO UPDATE SET
 quality = CASE WHEN ?2 IN ('all', 'format') THEN 'best' ELSE dl_user_settings.quality END,
 codec = CASE WHEN ?2 IN ('all', 'format') THEN 'avc1' ELSE dl_user_settings.codec END,
 container = CASE WHEN ?2 IN ('all', 'format') THEN 'mp4' ELSE dl_user_settings.container END,
 proxy_enabled = CASE WHEN ?2 IN ('all', 'proxy') THEN 0 ELSE dl_user_settings.proxy_enabled END,
 split_size = CASE WHEN ?2 IN ('all', 'split') THEN 2043000000 ELSE dl_user_settings.split_size END,
 subs_enabled = CASE WHEN ?2 IN ('all', 'subs') THEN 0 ELSE dl_user_settings.subs_enabled END,
 subs_auto = CASE WHEN ?2 IN ('all', 'subs') THEN 0 ELSE dl_user_settings.subs_auto END,
 subs_always_ask = CASE WHEN ?2 IN ('all', 'subs') THEN 0 ELSE dl_user_settings.subs_always_ask END,
 subs_lang = CASE WHEN ?2 IN ('all', 'subs') THEN 'en' ELSE dl_user_settings.subs_lang END,
 nsfw_blur = CASE WHEN ?2 IN ('all', 'nsfw') THEN 1 ELSE dl_user_settings.nsfw_blur END,
 mediainfo = CASE WHEN ?2 IN ('all', 'mediainfo') THEN 0 ELSE dl_user_settings.mediainfo END,
 keyboard = CASE WHEN ?2 IN ('all', 'keyboard') THEN '2x3' ELSE dl_user_settings.keyboard END,
 args_json = CASE WHEN ?2 IN ('all', 'args') THEN '{}' ELSE dl_user_settings.args_json END,
 updated_at = max(excluded.updated_at, dl_user_settings.updated_at + 1)
RETURNING user_id;
