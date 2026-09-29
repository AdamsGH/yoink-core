INSERT INTO dl_user_settings
 (user_id, quality, codec, container, audio_codec, send_as_file, gallery_zip, subs_lang, subs_auto, proxy_enabled, created_at, updated_at)
SELECT u.id, 'best', 'avc1', 'mp4', 'best', 0, 0, 'en', 0, ?2, ?4, ?4
FROM users u
WHERE u.id = ?1 AND ?5 = 1
  AND (u.role IN ('user', 'moderator', 'admin', 'owner') OR (?5 = 1 AND u.role = 'restricted'))
  AND (u.ban_until IS NULL OR u.ban_until <= ?4)
  AND COALESCE((SELECT updated_at FROM dl_user_settings WHERE user_id = ?1), 0) = ?3
ON CONFLICT(user_id) DO UPDATE SET
 proxy_enabled = excluded.proxy_enabled,
 updated_at = max(excluded.updated_at, dl_user_settings.updated_at + 1)
WHERE COALESCE(dl_user_settings.updated_at, 0) = ?3
RETURNING user_id;
