INSERT INTO download_log(user_id,url,domain,title,quality,status,error_msg,group_id,thread_id,message_id,file_size,file_count,created_at)
VALUES(?1,?2,?3,?4,'audio',?5,?6,?7,?8,?9,?10,1,unixepoch()*1000000);
