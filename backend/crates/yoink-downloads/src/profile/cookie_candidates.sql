-- Domain normalization and matching are applied in Rust so aliases, www hosts,
-- URL-form domains, and other accepted forms cannot be filtered out here.
SELECT id, user_id, domain, is_pool, updated_at
FROM cookies
WHERE is_valid = 1
  AND ((user_id = ?1 AND is_pool = 0) OR (?2 = 1 AND is_pool = 1))
ORDER BY is_pool ASC, id ASC;