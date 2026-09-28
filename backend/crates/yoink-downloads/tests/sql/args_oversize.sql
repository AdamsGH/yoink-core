UPDATE dl_user_settings
SET args_json = json_object('padding', replace(hex(zeroblob(35000)), '00', 'xx'))
WHERE user_id = ?1;
