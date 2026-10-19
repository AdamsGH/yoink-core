SELECT MAX(version) AS version, SUM(CASE WHEN success=0 THEN 1 ELSE 0 END) AS failed
FROM runtime_migrations;
