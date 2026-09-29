INSERT INTO dl_user_settings (user_id, quality, codec, container, audio_codec, send_as_file, gallery_zip, subs_lang, subs_auto, proxy_enabled, proxy_url, created_at, updated_at)
VALUES (?1, 'best', 'avc1', 'mp4', 'best', 0, 0, 'en', 0, ?2, ?3, 1700000000000000, ?4)
ON CONFLICT(user_id) DO UPDATE SET proxy_enabled = excluded.proxy_enabled, proxy_url = excluded.proxy_url, updated_at = excluded.updated_at;
