INSERT INTO users (id, role, language, theme, is_premium, created_at, updated_at, ban_until)
VALUES
    (42, 'restricted', 'en', 'macchiato', 1, 1700000000000000, 1700000000000000, NULL),
    (43, 'user', 'ru', 'macchiato', 1, 1700000000000000, 1700000000000000, NULL),
    (44, 'banned', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, NULL),
    (45, 'user', 'en', 'macchiato', 0, 1700000000000000, 9223372036854775807, NULL),
    (46, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, 9223372036854775807),
    (47, 'user', 'en', 'macchiato', 0, 1700000000000000, 1700000000000000, 1);
