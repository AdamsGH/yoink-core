UPDATE users
SET username = COALESCE(?1, username),
    first_name = COALESCE(?2, first_name),
    photo_url = COALESCE(?3, photo_url),
    updated_at = max(updated_at + 1, CAST(unixepoch('subsec') * 1000000 AS INTEGER))
WHERE id = ?4 AND updated_at < 9223372036854775807
RETURNING id, username, first_name, photo_url, role, language, theme, ban_until, created_at, updated_at;
