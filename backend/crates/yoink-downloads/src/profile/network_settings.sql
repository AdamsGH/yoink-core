SELECT COALESCE(proxy_enabled, 0) AS proxy_enabled, proxy_url,
       COALESCE(use_pool_cookies, 1) AS use_pool_cookies,
       COALESCE(youtube_auth_mode, 'cookies') AS youtube_auth_mode
FROM dl_user_settings WHERE user_id = ?1;