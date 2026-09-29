CREATE TABLE runtime_personal_cookie_mutation_watermarks (
 actor_id INTEGER NOT NULL CHECK(actor_id>0),
 message_id INTEGER NOT NULL CHECK(message_id>0 AND message_id<=2147483647),
 PRIMARY KEY(actor_id)
) STRICT;
