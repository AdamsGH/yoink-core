UPDATE dl_user_settings SET quality = 'not-a-quality', args_json = '["raw-args-secret-marker"]' WHERE user_id = ?1;
