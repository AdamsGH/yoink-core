UPDATE yoink_runtime._sqlx_migrations SET checksum = decode(repeat('00', 32), 'hex') WHERE version = $1;
