UPDATE users SET ban_until = CAST(unixepoch('subsec') * 1000000 AS INTEGER) + 3600000000 WHERE id = ?1;
