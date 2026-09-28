SELECT database_id, source_revision, acknowledged_revision
FROM stats_search_state WHERE singleton = 1;
