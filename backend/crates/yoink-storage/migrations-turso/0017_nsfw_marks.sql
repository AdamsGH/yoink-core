-- Durable actor-wide one-shot intent. Not a classification or cache property.
CREATE TABLE runtime_nsfw_marks (
    actor_id INTEGER PRIMARY KEY CHECK (actor_id > 0),
    FOREIGN KEY (actor_id) REFERENCES users(id) ON DELETE CASCADE
);

-- Retained after consumption, so a retried old Telegram command cannot re-arm.
CREATE TABLE runtime_nsfw_mark_events (
    actor_id INTEGER NOT NULL CHECK (actor_id > 0),
    bot_id INTEGER NOT NULL CHECK (bot_id > 0),
    chat_id INTEGER NOT NULL CHECK (chat_id <> 0),
    message_id INTEGER NOT NULL CHECK (message_id > 0),
    PRIMARY KEY (actor_id, bot_id, chat_id),
    FOREIGN KEY (actor_id) REFERENCES users(id) ON DELETE CASCADE
);
