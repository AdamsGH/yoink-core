SELECT
  COALESCE(s.quality, 'best') AS quality,
  COALESCE(s.codec, 'avc1') AS codec,
  COALESCE(s.container, 'mp4') AS container,
  COALESCE(s.proxy_enabled, false) AS proxy_enabled,
  s.proxy_url,
  COALESCE(s.keyboard, '2x3') AS keyboard,
  COALESCE(s.subs_enabled, false) AS subs_enabled,
  COALESCE(s.subs_auto, false) AS subs_auto,
  COALESCE(s.subs_always_ask, false) AS subs_always_ask,
  COALESCE(s.subs_lang, 'en') AS subs_lang,
  COALESCE(s.split_size, 2043000000) AS split_size,
  COALESCE(s.nsfw_blur, true) AS nsfw_blur,
  COALESCE(s.mediainfo, false) AS mediainfo,
  COALESCE(s.send_as_file, false) AS send_as_file,
  COALESCE(s.audio_codec, 'best') AS audio_codec,
  COALESCE(s.gallery_zip, false) AS gallery_zip,
  COALESCE(s.use_pool_cookies, true) AS use_pool_cookies,
  COALESCE(s.youtube_auth_mode, 'cookies') AS youtube_auth_mode,
  s.dm_topic_thread_id,
  CASE WHEN octet_length(COALESCE(s.args_json, '{}'::json)::text) <= 65536
       THEN COALESCE(s.args_json, '{}'::json) ELSE '{}'::json END AS args_json,
  octet_length(COALESCE(s.args_json, '{}'::json)::text) > 65536 AS args_json_too_large,
  s.updated_at
FROM public.users AS u
LEFT JOIN public.dl_user_settings AS s ON s.user_id = u.id
WHERE u.id = $1
