CREATE TABLE runtime_text_effects(state TEXT,receipt INTEGER,identity TEXT);
CREATE TABLE runtime_migrations(version INTEGER,success INTEGER);
INSERT INTO runtime_migrations VALUES(27,1);
INSERT INTO runtime_text_effects VALUES('confirmed',23,
 '{"actor":42,"chat":42,"message":22,"text":"SENTINEL_PRIVATE_BODY","payload":{"application":{"incoming":"/tldr https://example.org/ nobullshit в двух словах"}}}');
