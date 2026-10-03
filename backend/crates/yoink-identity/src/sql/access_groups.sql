SELECT id FROM groups WHERE id < 0 AND id > ?1 ORDER BY id LIMIT 128;
