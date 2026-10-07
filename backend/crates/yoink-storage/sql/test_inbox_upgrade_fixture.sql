INSERT INTO users(id,created_at,updated_at) VALUES(41,1,1),(42,1,1);
INSERT INTO inbox_items(id,user_id,url,normalized_url,title,summary,status,content_text,created_at,modified_at,archived_at,tg_chat_id,tg_reply_message_id)
VALUES
(101,41,'https://fixture.invalid/pending','https://fixture.invalid/pending','Pending','Original summary','pending',NULL,1,110,NULL,41,501),
(102,41,'https://fixture.invalid/enriched','https://fixture.invalid/enriched','Enriched',NULL,'enriched','Original body',2,120,NULL,41,502),
(103,41,'https://fixture.invalid/archived','https://fixture.invalid/archived','Archived',NULL,'pending',NULL,3,130,131,41,503),
(104,41,'https://fixture.invalid/classified','https://fixture.invalid/classified','Classified','Preserved summary','classified','Preserved body',4,140,NULL,41,504),
(105,42,'https://fixture.invalid/failed','https://fixture.invalid/failed','Failed',NULL,'failed',NULL,5,150,NULL,NULL,NULL);
INSERT INTO inbox_categories(id,owner_user_id,name,normalized_name,slug,created_at)
VALUES(201,41,'Manual','manual','manual',1);
INSERT INTO inbox_item_categories(item_id,category_id,attached_by,attached_by_user_id,attached_at)
VALUES(102,201,'manual',41,1);
INSERT INTO inbox_gh_folders(id,user_id,name,slug,description,created_at,gh_list_id,gh_list_slug,is_pinned,sort_order)
VALUES(301,41,'Preserved list','preserved-list','Original description',1,'L_fixture','remote-list',1,7);
