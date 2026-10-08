SELECT json_array(
 (SELECT json_group_array(json_array(user_id,lang,granted_by,granted_at)) FROM insight_access),
 (SELECT json_group_array(json_array(user_id,lang,tldr_model,github_token,use_search,github_token_public_repo)) FROM insight_user_settings),
 (SELECT json_group_array(json_array(user_id,provider,base_url,api_key,model,models_json,models_fetched_at,tested_at,test_error,created_at,updated_at)) FROM insight_user_byok),
 (SELECT json_group_array(json_array(id,user_id,aliases,prompt,created_at,domains,target_alias)) FROM insight_tldr_aliases),
 (SELECT json_group_array(json_array(user_id,command,prompt,updated_at)) FROM insight_user_prompts),
 (SELECT json_group_array(json_array(id,content_key,lang,command,result,created_at,expires_at)) FROM insight_summary_cache),
 (SELECT json_group_array(json_array(id,user_id,command,video_id,lang,status,error_code,created_at,content_chars,video_seconds,alias_key,route)) FROM insight_usage_log)
) AS snapshot;
