INSERT INTO users (id, role, language, theme, is_premium, created_at, updated_at, ban_until) VALUES
(8000000000201, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, NULL),
(8000000000202, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, NULL),
(8000000000203, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, NULL),
(8000000000204, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, NULL),
(8000000000205, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, NULL);

INSERT INTO dl_user_settings
(user_id, quality, codec, container, proxy_enabled, proxy_url, keyboard, subs_enabled, subs_auto,
 subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip,
 use_pool_cookies, youtube_auth_mode, dm_topic_thread_id, args_json, created_at, updated_at)
VALUES
(8000000000201, 'best', 'avc1', 'mp4', 0, NULL, '2x3', 0, 0, 0, 'en',
 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', NULL, '{}', 1700000000000000, 1700000000000000),
(8000000000203, 'best', 'avc1', 'mp4', 0, NULL, '2x3', 0, 0, 0, 'en',
 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', NULL, '{}', 1700000000000000, 1700000000000000);
