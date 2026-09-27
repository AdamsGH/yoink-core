INSERT INTO public.bot_settings (key, value, updated_at)
VALUES ('insight_byok_enabled', $1, now())
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at;
