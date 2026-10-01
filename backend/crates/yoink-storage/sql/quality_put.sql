INSERT INTO runtime_quality_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,request_json,selected,operation_id,linked_cut_id)
VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
ON CONFLICT(actor_id,bot_id,chat_id) DO UPDATE SET
 id=excluded.id,revision=excluded.revision,stage=excluded.stage,
 origin_message_id=excluded.origin_message_id,last_message_id=excluded.last_message_id,
 request_json=excluded.request_json,selected=excluded.selected,operation_id=excluded.operation_id,
 linked_cut_id=excluded.linked_cut_id;
