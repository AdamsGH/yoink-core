INSERT INTO runtime_quality_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,request_json,selected,operation_id) VALUES
(801,1,801,'00000000-0000-4000-8000-000000000801',1,'choosing',11,11,'{}',NULL,NULL),
(802,1,802,'00000000-0000-4000-8000-000000000802',2,'confirming',12,12,'{}','audio',NULL),
(803,1,803,'00000000-0000-4000-8000-000000000803',3,'submitted',13,13,'{}','audio','00000000-0000-4000-8000-000000000899'),
(804,1,804,'00000000-0000-4000-8000-000000000804',2,'cancelled',14,14,'{}',NULL,NULL);
INSERT INTO runtime_cut_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id,selected) VALUES
(811,1,811,'00000000-0000-4000-8000-000000000811',1,'start',21,21,'https://example.invalid/a',NULL,NULL,NULL,NULL,NULL),
(815,1,815,'00000000-0000-4000-8000-000000000815',1,'end',30,30,'https://example.invalid/e',1,1,NULL,NULL,NULL),
(816,1,816,'00000000-0000-4000-8000-000000000816',2,'ready',31,31,'https://example.invalid/f',1,1,3,NULL,NULL),
(817,1,817,'00000000-0000-4000-8000-000000000817',3,'quality',32,32,'https://example.invalid/g',1,1,3,NULL,NULL),
(812,1,812,'00000000-0000-4000-8000-000000000812',2,'confirming',22,24,'https://example.invalid/b',2,2,4,NULL,'720'),
(813,1,813,'00000000-0000-4000-8000-000000000813',2,'submitted',25,27,'https://example.invalid/c',1,1,3,'00000000-0000-4000-8000-000000000899','best'),
(814,1,814,'00000000-0000-4000-8000-000000000814',2,'cancelled',28,29,'https://example.invalid/d',NULL,NULL,NULL,NULL,NULL);
