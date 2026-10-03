UPDATE users
SET language = COALESCE(?1, language),
    theme = COALESCE(?2, theme),
    updated_at = max(updated_at + 1, CAST(unixepoch('subsec') * 1000000 AS INTEGER))
WHERE id = ?3 AND updated_at < 9223372036854775807
RETURNING id, language, theme;
