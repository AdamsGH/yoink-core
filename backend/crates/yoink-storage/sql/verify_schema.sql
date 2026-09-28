SELECT type, name, tbl_name, sql
FROM sqlite_master
WHERE name NOT LIKE 'sqlite_%'
  AND name NOT GLOB '__turso_internal_*'
  AND type IN ('table', 'index', 'trigger', 'view')
ORDER BY type, name;
