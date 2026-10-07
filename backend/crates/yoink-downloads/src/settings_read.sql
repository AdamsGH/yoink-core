SELECT
 CASE WHEN length(CAST(COALESCE(s.quality, 'best') AS BLOB)) <= 32 THEN COALESCE(s.quality, 'best') END,
 CASE WHEN length(CAST(COALESCE(s.codec, 'avc1') AS BLOB)) <= 16 THEN COALESCE(s.codec, 'avc1') END,
 CASE WHEN length(CAST(COALESCE(s.container, 'mp4') AS BLOB)) <= 8 THEN COALESCE(s.container, 'mp4') END,
 CASE WHEN length(CAST(COALESCE(s.audio_codec, 'best') AS BLOB)) <= 16 THEN COALESCE(s.audio_codec, 'best') END,
 CASE WHEN COALESCE(s.send_as_file, 0) = 0 THEN 'false' ELSE 'true' END,
 CASE WHEN COALESCE(s.gallery_zip, 0) = 0 THEN 'false' ELSE 'true' END,
 CASE WHEN length(CAST(COALESCE(s.subs_lang, 'en') AS BLOB)) <= 8 THEN COALESCE(s.subs_lang, 'en') END,
 CASE WHEN COALESCE(s.subs_auto, 0) = 0 THEN 'false' ELSE 'true' END,
 CASE WHEN COALESCE(s.nsfw_blur, 1) = 0 THEN 'false' ELSE 'true' END,
 CASE WHEN COALESCE(s.subs_enabled, 0) = 0 THEN 'false' ELSE 'true' END,
 CASE WHEN COALESCE(s.subs_always_ask, 0) = 0 THEN 'false' ELSE 'true' END,
 CAST(COALESCE(s.split_size, 2043000000) AS TEXT),
 CASE WHEN length(CAST(COALESCE(s.keyboard, '2x3') AS BLOB)) <= 8 THEN COALESCE(s.keyboard, '2x3') END,
 CASE WHEN COALESCE(s.mediainfo, 0) = 0 THEN 'false' ELSE 'true' END,
 CASE WHEN COALESCE(s.use_pool_cookies, 1) = 0 THEN 'false' ELSE 'true' END,
 COALESCE(s.updated_at, 0)
FROM users u LEFT JOIN dl_user_settings s ON s.user_id = u.id
WHERE u.id = ?1;
