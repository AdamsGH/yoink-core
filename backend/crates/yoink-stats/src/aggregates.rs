use crate::{
    Error,
    analytics::{Params, iso},
};
use chrono::Utc;
use serde_json::{Value, json};
use yoink_db::{Executor, query};
pub(crate) async fn aggregate(
    conn: &turso::Connection,
    endpoint: &str,
    p: &Params,
    limit: usize,
) -> Result<Option<Value>, Error> {
    let start = p
        .start
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_micros());
    let end = p
        .end
        .and_then(|d| d.succ_opt())
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_micros());
    let since = p
        .days
        .map(|d| (Utc::now() - chrono::Duration::days(d)).timestamp_micros());
    let start = match (start, since) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let source = "WITH m AS (SELECT * FROM stats_messages WHERE chat_id=?1 AND (?2 IS NULL OR date>=?2) AND (?3 IS NULL OR date<?3) AND (?4 IS NULL OR from_user=?4) AND (?5 IS NULL OR msg_type=?5)) ";
    auxiliary(conn, endpoint, p, limit, start, end, source).await
}
pub(crate) async fn profile<E: Executor>(conn: &E, uid: i64) -> Result<Value, Error> {
    let row=query("SELECT coalesce(n.username,u.username) username,coalesce(n.display_name,u.first_name) display_name,u.photo_url FROM (SELECT ? AS id) ids LEFT JOIN users u ON u.id=ids.id LEFT JOIN stats_user_latest_name n ON n.user_id=ids.id").bind(uid).fetch_one(conn).await?;
    Ok(
        json!({"user_id":uid,"username":row.try_get::<Option<String>,_>(0)?,"display_name":row.try_get::<Option<String>,_>(1)?,"has_photo":row.try_get::<Option<String>,_>(2)?.is_some()}),
    )
}

async fn auxiliary<E: Executor>(
    conn: &E,
    endpoint: &str,
    p: &Params,
    limit: usize,
    start: Option<i64>,
    end: Option<i64>,
    source: &str,
) -> Result<Option<Value>, Error> {
    let now = Utc::now();
    let since = p
        .days
        .map(|d| (now - chrono::Duration::days(d)).timestamp_micros());
    let result = match endpoint {
        "chat-admins" => {
            let rows = query(
                "SELECT user_id,status FROM stats_chat_admins WHERE chat_id=? ORDER BY user_id",
            )
            .bind(p.chat_id)
            .fetch_all(conn)
            .await?;
            let mut output = Vec::new();
            for r in rows {
                output.push(
                    json!({"user_id":r.try_get::<i64,_>(0)?,"status":r.try_get::<String,_>(1)?}),
                );
            }
            json!(output)
        }
        "top-reactions" => {
            let rows=query("SELECT user_id,count(*) n FROM stats_reactions WHERE chat_id=? AND (? IS NULL OR date>=?) GROUP BY user_id ORDER BY n DESC,user_id LIMIT ?").bind(p.chat_id).bind(since).bind(since).bind(limit as i64).fetch_all(conn).await?;
            let mut givers = Vec::new();
            for r in rows {
                let mut v = profile(conn, r.try_get(0)?).await?;
                v["reaction_count"] = json!(r.try_get::<i64, _>(1)?);
                givers.push(v);
            }
            let rows=query("SELECT reaction_key,reaction_type,count(*) n FROM stats_reactions WHERE chat_id=? AND (? IS NULL OR date>=?) GROUP BY reaction_key,reaction_type ORDER BY n DESC,reaction_key,reaction_type LIMIT ?").bind(p.chat_id).bind(since).bind(since).bind(limit as i64).fetch_all(conn).await?;
            let mut emoji = Vec::new();
            for r in rows {
                emoji.push(json!({"reaction_key":r.try_get::<String,_>(0)?,"reaction_type":r.try_get::<String,_>(1)?,"count":r.try_get::<i64,_>(2)?}));
            }
            json!({"top_givers":givers,"top_emoji":emoji})
        }
        "member-events" => {
            let rows=query("SELECT strftime('%Y-%m-%d',date/1000000.0,'unixepoch') day,sum(CASE WHEN event='joined' THEN 1 ELSE 0 END),sum(CASE WHEN event='left' THEN 1 ELSE 0 END) FROM stats_user_events WHERE chat_id=? AND (? IS NULL OR date>=?) GROUP BY day ORDER BY day").bind(p.chat_id).bind(since).bind(since).fetch_all(conn).await?;
            let mut output = Vec::new();
            for r in rows {
                output.push(json!({"date":r.try_get::<String,_>(0)?,"joined":r.try_get::<i64,_>(1)?,"left":r.try_get::<i64,_>(2)?}));
            }
            json!(output)
        }
        "members" => {
            let statement = format!(
                "{source}, ma AS (SELECT from_user,count(*) n,min(date) first,max(date) last FROM m WHERE from_user IS NOT NULL GROUP BY from_user), ra AS (SELECT user_id,count(*) n,max(date) last FROM stats_reactions WHERE chat_id=?1 AND (?6 IS NULL OR date>=?6) GROUP BY user_id), ids AS (SELECT from_user uid FROM stats_messages WHERE chat_id=?1 AND from_user IS NOT NULL UNION SELECT user_id FROM stats_group_members WHERE chat_id=?1), member_rows AS (SELECT ids.uid,coalesce(ma.n,0) messages,coalesce(ra.n,0) reactions,ma.first,CASE WHEN coalesce(ma.last,0)>coalesce(ra.last,0) THEN ma.last ELSE ra.last END last,g.in_chat,g.synced_at FROM ids LEFT JOIN ma ON ma.from_user=ids.uid LEFT JOIN ra ON ra.user_id=ids.uid LEFT JOIN stats_group_members g ON g.chat_id=?1 AND g.user_id=ids.uid) SELECT * FROM member_rows ORDER BY CASE WHEN last>0 THEN last ELSE NULL END DESC,uid LIMIT 1000"
            );
            let rows = query(&statement)
                .bind(p.chat_id)
                .bind(start)
                .bind(end)
                .bind(p.user_id)
                .bind(p.msg_type.clone())
                .bind(since)
                .fetch_all(conn)
                .await?;
            let cutoff = since.unwrap_or((now - chrono::Duration::days(90)).timestamp_micros());
            let mut output = Vec::new();
            for r in rows {
                let mut v = profile(conn, r.try_get("uid")?).await?;
                let last = r.try_get::<Option<i64>, _>("last")?.filter(|d| *d > 0);
                v["message_count"] = json!(r.try_get::<i64, _>("messages")?);
                v["reaction_count"] = json!(r.try_get::<i64, _>("reactions")?);
                v["first_seen_at"] = iso(r.try_get("first")?)?;
                v["last_active_at"] = iso(last)?;
                v["is_active"] = json!(last.is_some_and(|d| d > cutoff));
                v["in_chat"] = json!(r.try_get::<Option<bool>, _>("in_chat")?);
                v["synced_at"] = iso(r.try_get("synced_at")?)?;
                output.push(v);
            }
            json!(output)
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}
