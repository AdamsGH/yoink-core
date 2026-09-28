SELECT
  COALESCE(s.quality, 'best') AS quality,
  COALESCE(s.codec, 'avc1') AS codec,
  COALESCE(s.container, 'mp4') AS container,
  COALESCE(s.proxy_enabled, 0) AS proxy_enabled,
  s.proxy_url,
  COALESCE(s.keyboard, '2x3') AS keyboard,
  COALESCE(s.subs_enabled, 0) AS subs_enabled,
  COALESCE(s.subs_auto, 0) AS subs_auto,
  COALESCE(s.subs_always_ask, 0) AS subs_always_ask,
  COALESCE(s.subs_lang, 'en') AS subs_lang,
  COALESCE(s.split_size, 2043000000) AS split_size,
  COALESCE(s.nsfw_blur, 1) AS nsfw_blur,
  COALESCE(s.mediainfo, 0) AS mediainfo,
  COALESCE(s.send_as_file, 0) AS send_as_file,
  COALESCE(s.audio_codec, 'best') AS audio_codec,
  COALESCE(s.gallery_zip, 0) AS gallery_zip,
  COALESCE(s.use_pool_cookies, 1) AS use_pool_cookies,
  COALESCE(s.youtube_auth_mode, 'cookies') AS youtube_auth_mode,
  s.dm_topic_thread_id,
  CASE WHEN length(CAST(COALESCE(s.args_json, '{}') AS BLOB)) <= 65536
       THEN COALESCE(s.args_json, '{}') ELSE '{}' END AS args_json,
  length(CAST(COALESCE(s.args_json, '{}') AS BLOB)) > 65536 AS args_json_too_large,
  s.updated_at
FROM users AS u
LEFT JOIN dl_user_settings AS s ON s.user_id = u.id
WHERE u.id = ?1;