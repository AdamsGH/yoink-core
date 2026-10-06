SELECT COALESCE((SELECT value FROM bot_settings WHERE key='nsfw_rules_revision'),'0');
