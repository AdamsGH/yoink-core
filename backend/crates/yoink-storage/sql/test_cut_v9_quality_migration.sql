INSERT INTO runtime_cut_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id) VALUES
 (7,9,7,'38e4f36a-14da-4f7b-8d1d-12d2191d8db1',4,'confirming',11,13,'https://example.test/confirming',1.0,2.0,8.0,NULL),
 (8,9,8,'48e4f36a-14da-4f7b-8d1d-12d2191d8db2',5,'ready',21,23,'https://example.test/ready',2.0,3.0,9.0,NULL),
 (10,9,10,'58e4f36a-14da-4f7b-8d1d-12d2191d8db3',6,'submitted',31,33,'https://example.test/submitted',3.0,4.0,10.0,'68e4f36a-14da-4f7b-8d1d-12d2191d8db4'),
 (12,9,12,'78e4f36a-14da-4f7b-8d1d-12d2191d8db5',7,'cancelled',41,43,'https://example.test/cancelled',4.0,NULL,NULL,NULL),
 (13,9,13,'88e4f36a-14da-4f7b-8d1d-12d2191d8db6',1,'start',51,53,'https://example.test/start',0.0,NULL,-1.0,NULL),
 (14,9,14,'98e4f36a-14da-4f7b-8d1d-12d2191d8db7',2,'end',61,63,'https://example.test/end',0.0,NULL,-2.0,NULL),
 (15,9,15,'a8e4f36a-14da-4f7b-8d1d-12d2191d8db8',3,'cancelled',71,73,'https://example.test/cancelled-negative',0.0,NULL,-3.0,NULL);
