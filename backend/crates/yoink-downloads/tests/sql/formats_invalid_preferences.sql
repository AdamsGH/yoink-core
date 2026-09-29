UPDATE dl_user_settings
SET quality = 'not-a-quality', codec = 'not-a-codec', container = 'invalid',
    audio_codec = 'invalid', send_as_file = 0
WHERE user_id = 42;
