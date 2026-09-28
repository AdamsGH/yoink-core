UPDATE dl_user_settings
SET subs_lang = ?2, subs_auto = ?3, subs_enabled = ?4
WHERE user_id = ?1;
