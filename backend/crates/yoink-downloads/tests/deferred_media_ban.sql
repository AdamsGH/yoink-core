UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42;
