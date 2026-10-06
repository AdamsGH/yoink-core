INSERT INTO users(id,role,language,created_at,updated_at) VALUES
 (42,'owner','en',1000000,1000000),(43,'moderator','ru',1000000,1000000),(44,'user','en',1000000,1000000),
 (45,'banned','en',1000000,1000000);
INSERT INTO groups(id,enabled,nsfw_allowed,created_at) VALUES(-100,1,0,1000000);
INSERT INTO nsfw_domains(domain,note,created_at) VALUES('legacy.example','imported note',1000000);
INSERT INTO nsfw_keywords(keyword,note,created_at) VALUES('sensitivecontent','imported keyword',1000000);
