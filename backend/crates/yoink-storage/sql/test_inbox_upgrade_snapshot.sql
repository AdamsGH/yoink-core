SELECT json_object(
 'items', (SELECT json_group_array(json_array(id,user_id,url,normalized_url,title,summary,status,content_text,created_at,modified_at,archived_at,tg_chat_id,tg_reply_message_id)) FROM (SELECT * FROM inbox_items ORDER BY id)),
 'categories', (SELECT json_group_array(json_array(id,owner_user_id,name,normalized_name,slug,created_at)) FROM (SELECT * FROM inbox_categories ORDER BY id)),
 'assignments', (SELECT json_group_array(json_array(item_id,category_id,attached_by,attached_by_user_id,attached_at)) FROM (SELECT * FROM inbox_item_categories ORDER BY item_id,category_id)),
 'folders', (SELECT json_group_array(json_array(id,user_id,name,slug,description,created_at,gh_list_id,gh_list_slug,is_pinned,sort_order)) FROM (SELECT * FROM inbox_gh_folders ORDER BY id))
) AS snapshot;
