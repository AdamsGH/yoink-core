SELECT COALESCE(proxy_enabled, false) AS proxy_enabled, proxy_url,
       COALESCE(use_pool_cookies, true) AS use_pool_cookies,
       COALESCE(youtube_auth_mode, 'cookies') AS youtube_auth_mode
FROM public.dl_user_settings WHERE user_id = $1
