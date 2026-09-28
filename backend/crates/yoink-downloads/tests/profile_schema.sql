CREATE TABLE users (id INTEGER PRIMARY KEY);
CREATE TABLE dl_user_settings (
    user_id INTEGER PRIMARY KEY,
    quality TEXT,
    codec TEXT,
    container TEXT,
    proxy_enabled INTEGER,
    proxy_url TEXT,
    keyboard TEXT,
    subs_enabled INTEGER,
    subs_auto INTEGER,
    subs_always_ask INTEGER,
    subs_lang TEXT,
    split_size INTEGER,
    nsfw_blur INTEGER,
    mediainfo INTEGER,
    send_as_file INTEGER,
    audio_codec TEXT,
    gallery_zip INTEGER,
    use_pool_cookies INTEGER,
    youtube_auth_mode TEXT,
    dm_topic_thread_id INTEGER,
    args_json TEXT,
    updated_at INTEGER
);
CREATE TABLE cookies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    domain TEXT NOT NULL,
    content TEXT NOT NULL,
    is_valid INTEGER NOT NULL,
    is_pool INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
