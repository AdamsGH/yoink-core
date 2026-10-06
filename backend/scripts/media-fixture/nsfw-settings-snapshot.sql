SELECT json_object('nsfw_blur',nsfw_blur,'send_as_file',send_as_file,'quality',quality,'codec',codec,'container',container,'updated_at',updated_at) FROM dl_user_settings WHERE user_id=42;
