-- JSON is produced only from the validated typed patch; null means no change.
INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, proxy_url, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, gallery_zip, use_pool_cookies, youtube_auth_mode, created_at, updated_at)
SELECT id, COALESCE(json_extract(?2, '$.quality'), 'best'), COALESCE(json_extract(?2, '$.codec'), 'avc1'), COALESCE(json_extract(?2, '$.container'), 'mp4'), COALESCE(json_extract(?2, '$.proxy_enabled'), 0), COALESCE(json_extract(?2, '$.proxy_url'), NULL), COALESCE(json_extract(?2, '$.keyboard'), '2x3'), COALESCE(json_extract(?2, '$.subs_enabled'), 0), COALESCE(json_extract(?2, '$.subs_auto'), 0), COALESCE(json_extract(?2, '$.subs_always_ask'), 0), COALESCE(json_extract(?2, '$.subs_lang'), 'en'), COALESCE(json_extract(?2, '$.split_size'), 2043000000), COALESCE(json_extract(?2, '$.nsfw_blur'), 1), COALESCE(json_extract(?2, '$.mediainfo'), 0), COALESCE(json_extract(?2, '$.send_as_file'), 0), COALESCE(json_extract(?2, '$.gallery_zip'), 0), COALESCE(json_extract(?2, '$.use_pool_cookies'), 1), COALESCE(json_extract(?2, '$.youtube_auth_mode'), 'cookies'), ?4, ?4 FROM users
WHERE id = ?1
 AND (role IN ('restricted', 'user', 'moderator', 'admin', 'owner'))
 AND (ban_until IS NULL OR ban_until <= ?4)
 AND COALESCE((SELECT updated_at FROM dl_user_settings WHERE user_id = ?1), 0) BETWEEN 0 AND 9223372036854775806
 AND (?3 IS NULL OR COALESCE((SELECT updated_at FROM dl_user_settings WHERE user_id = ?1), 0) = ?3)
ON CONFLICT(user_id) DO UPDATE SET
 quality = COALESCE(json_extract(?2, '$.quality'), dl_user_settings.quality),
 codec = COALESCE(json_extract(?2, '$.codec'), dl_user_settings.codec),
 container = COALESCE(json_extract(?2, '$.container'), dl_user_settings.container),
 proxy_enabled = COALESCE(json_extract(?2, '$.proxy_enabled'), dl_user_settings.proxy_enabled),
 proxy_url = COALESCE(json_extract(?2, '$.proxy_url'), dl_user_settings.proxy_url),
 keyboard = COALESCE(json_extract(?2, '$.keyboard'), dl_user_settings.keyboard),
 subs_enabled = COALESCE(json_extract(?2, '$.subs_enabled'), dl_user_settings.subs_enabled),
 subs_auto = COALESCE(json_extract(?2, '$.subs_auto'), dl_user_settings.subs_auto),
 subs_always_ask = COALESCE(json_extract(?2, '$.subs_always_ask'), dl_user_settings.subs_always_ask),
 subs_lang = COALESCE(json_extract(?2, '$.subs_lang'), dl_user_settings.subs_lang),
 split_size = COALESCE(json_extract(?2, '$.split_size'), dl_user_settings.split_size),
 nsfw_blur = COALESCE(json_extract(?2, '$.nsfw_blur'), dl_user_settings.nsfw_blur),
 mediainfo = COALESCE(json_extract(?2, '$.mediainfo'), dl_user_settings.mediainfo),
 send_as_file = COALESCE(json_extract(?2, '$.send_as_file'), dl_user_settings.send_as_file),
 gallery_zip = COALESCE(json_extract(?2, '$.gallery_zip'), dl_user_settings.gallery_zip),
 use_pool_cookies = COALESCE(json_extract(?2, '$.use_pool_cookies'), dl_user_settings.use_pool_cookies),
 youtube_auth_mode = COALESCE(json_extract(?2, '$.youtube_auth_mode'), dl_user_settings.youtube_auth_mode),
 updated_at = max(excluded.updated_at, dl_user_settings.updated_at + 1)
RETURNING user_id;
