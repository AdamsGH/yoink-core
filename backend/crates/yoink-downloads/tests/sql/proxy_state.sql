SELECT quality, proxy_url, created_at, proxy_enabled, updated_at,
       (SELECT count(*) FROM cookies WHERE user_id = ?1) AS cookie_count,
       (SELECT count(*) FROM download_log WHERE user_id = ?1) AS download_count
FROM dl_user_settings WHERE user_id = ?1;
