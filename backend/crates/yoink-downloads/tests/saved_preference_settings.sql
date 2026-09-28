INSERT INTO dl_user_settings (
  user_id, quality, codec, container, proxy_enabled, keyboard,
  subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size,
  nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip,
  use_pool_cookies, youtube_auth_mode, args_json, created_at, updated_at
) VALUES (
  42, '720', 'h264', 'mp4', 0, '2x3',
  1, 1, 0, 'submark', 2043000000,
  1, 0, 1, 'opus', 0,
  1, 'cookies', '{"raw_args":"raw-args-secret-marker"}',
  1700000000000000, 1700000000000000
);
