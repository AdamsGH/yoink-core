SELECT
  CASE WHEN length(CAST(COALESCE(s.quality, 'best') AS BLOB)) <= 32 THEN COALESCE(s.quality, 'best') END AS quality,
  CASE WHEN length(CAST(COALESCE(s.codec, 'avc1') AS BLOB)) <= 16 THEN COALESCE(s.codec, 'avc1') END AS codec,
  CASE WHEN length(CAST(COALESCE(s.container, 'mp4') AS BLOB)) <= 8 THEN COALESCE(s.container, 'mp4') END AS container,
  COALESCE(s.send_as_file, 0) AS send_as_file,
  CASE WHEN length(CAST(COALESCE(s.audio_codec, 'best') AS BLOB)) <= 16 THEN COALESCE(s.audio_codec, 'best') END AS audio_codec,
  COALESCE(s.gallery_zip, 0) AS gallery_zip
FROM users AS u
LEFT JOIN dl_user_settings AS s ON s.user_id = u.id
WHERE u.id = ?1;
