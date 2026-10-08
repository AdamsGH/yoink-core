INSERT INTO users(id,role,created_at,updated_at) VALUES(42,'user',1700000000000000,1700000000000000);
INSERT INTO insight_access(user_id,lang,granted_by,granted_at) VALUES(42,'de',42,1700000000000000);
INSERT INTO insight_user_settings(user_id,lang,tldr_model,github_token,use_search,github_token_public_repo)
VALUES(42,'de','fixture-model','fixture-read-only',1,'fixture-public-repo');
INSERT INTO insight_user_byok(user_id,provider,base_url,api_key,model,models_json,models_fetched_at,tested_at,test_error,created_at,updated_at)
VALUES(42,'gemini','https://generativelanguage.googleapis.com/v1beta/openai','fixture-only-key','fixture-model','[]',1700000000000000,1700000000000000,NULL,1700000000000000,1700000000000000);
INSERT INTO insight_tldr_aliases(user_id,aliases,prompt,domains,target_alias)
VALUES(42,'tech-news','Fixture prompt','*.example.[co][om]','max');
INSERT INTO insight_user_prompts(user_id,command,prompt) VALUES(42,'summary','Fixture summary prompt');
INSERT INTO insight_summary_cache(content_key,lang,command,result,expires_at)
VALUES('fixture-video','de','tldr:max:v3','Fixture cached result',1900000000000000);
INSERT INTO insight_usage_log(user_id,command,video_id,lang,status,created_at,content_chars,video_seconds,alias_key,route)
VALUES(42,'tldr:max','fixture-video','de','cached',1700000000000000,1000,0,'max','byok');
