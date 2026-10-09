use crate::{Error, Stats};
use serde_json::Value;
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_db::query;

fn integer(v: &Value, field: &str) -> Result<i64, Error> {
    v[field].as_i64().ok_or(Error::Invalid)
}
fn seconds(v: i64) -> Result<i64, Error> {
    v.checked_mul(1_000_000).ok_or(Error::Invalid)
}
fn text<'a>(v: &'a Value, field: &str) -> Option<&'a str> {
    v[field].as_str()
}
async fn revision<E: yoink_db::Executor>(conn: &E) -> Result<i64, Error> {
    Ok(
        query("SELECT revision FROM stats_runtime_revision WHERE id=1")
            .fetch_one(conn)
            .await?
            .try_get(0)?,
    )
}
async fn unchanged<E: yoink_db::Executor>(
    conn: &E,
    kind: &str,
    chat: i64,
    subject: i64,
    revision: i64,
) -> Result<bool, Error> {
    Ok(query("SELECT 1 FROM stats_subject_revisions WHERE kind=? AND chat_id=? AND subject_id=? AND revision>?").bind(kind).bind(chat).bind(subject).bind(revision).fetch_optional(conn).await?.is_none())
}
#[expect(clippy::too_many_arguments)]
async fn watermark(
    tx: &Transaction<'_>,
    bot: i64,
    kind: &str,
    chat: i64,
    subject: i64,
    actor: i64,
    date: i64,
    update: i64,
) -> Result<bool, Error> {
    let old = query("SELECT event_date,update_id,bot_id FROM stats_ingestion_watermarks WHERE kind=? AND chat_id=? AND subject_id=? AND actor_id=?")
        .bind(kind).bind(chat).bind(subject).bind(actor).fetch_optional(&**tx).await?;
    if let Some(row) = old {
        let previous = row.try_get::<i64, _>(0)?;
        if date < previous
            || (date == previous
                && (bot != row.try_get::<i64, _>(2)? || update <= row.try_get::<i64, _>(1)?))
        {
            return Ok(false);
        }
    }
    query("INSERT INTO stats_ingestion_watermarks VALUES (?,?,?,?,?,?,?) ON CONFLICT(kind,chat_id,subject_id,actor_id) DO UPDATE SET bot_id=excluded.bot_id,event_date=excluded.event_date,update_id=excluded.update_id")
        .bind(bot).bind(kind).bind(chat).bind(subject).bind(actor).bind(date).bind(update).execute(&**tx).await?;
    Ok(true)
}
async fn name(
    tx: &Transaction<'_>,
    bot: i64,
    user: &Value,
    date: i64,
    update: i64,
) -> Result<bool, Error> {
    let uid = integer(user, "id")?;
    if uid <= 0 {
        return Err(Error::Invalid);
    }
    if [
        text(user, "username"),
        text(user, "first_name"),
        text(user, "last_name"),
    ]
    .into_iter()
    .flatten()
    .all(|s| s.trim().is_empty())
    {
        return Ok(false);
    }
    query("INSERT INTO stats_name_baselines(user_id,event_floor) SELECT ?,coalesce(max(date),0)/1000000*1000000 FROM stats_user_names WHERE user_id=? ON CONFLICT(user_id) DO NOTHING").bind(uid).bind(uid).execute(&**tx).await?;
    let floor =
        query("SELECT max(event_floor,coalesce(refresh_floor,event_floor)) FROM stats_name_baselines WHERE user_id=?")
            .bind(uid)
            .fetch_one(&**tx)
            .await?
            .try_get::<i64, _>(0)?;
    if bot != 0 && date < floor {
        return Ok(false);
    }
    if bot != 0 && !watermark(tx, bot, "name", 0, uid, 0, date, update).await? {
        return Ok(false);
    }
    let display = [text(user, "first_name"), text(user, "last_name")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let display = if display.trim().is_empty() {
        query("SELECT display_name FROM stats_user_latest_name WHERE user_id=?")
            .bind(uid)
            .fetch_optional(&**tx)
            .await?
            .map(|r| r.try_get::<Option<String>, _>(0))
            .transpose()?
            .flatten()
    } else {
        Some(display)
    };
    let latest = query("SELECT date,username,display_name FROM stats_user_names WHERE user_id=? ORDER BY date DESC LIMIT 1")
        .bind(uid)
        .fetch_optional(&**tx)
        .await?;
    if let Some(latest) = &latest
        && latest.try_get::<Option<String>, _>(1)?.as_deref() == text(user, "username")
        && latest.try_get::<Option<String>, _>(2)? == display
    {
        if bot == 0 {
            query("UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1")
                .execute(&**tx)
                .await?;
            query("INSERT INTO stats_subject_revisions SELECT 'name',0,?,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision").bind(uid).execute(&**tx).await?;
        }
        return Ok(true);
    }
    let latest = latest.map(|r| r.try_get::<i64, _>(0)).transpose()?;
    let observed = chrono::Utc::now().timestamp_micros().max(
        latest
            .map(|d| d.checked_add(1).ok_or(Error::Database))
            .transpose()?
            .unwrap_or(0),
    );
    query("INSERT INTO stats_user_names(user_id,date,username,display_name) VALUES(?,?,?,?)")
        .bind(uid)
        .bind(observed)
        .bind(text(user, "username"))
        .bind(display)
        .execute(&**tx)
        .await?;
    Ok(true)
}
async fn refreshed_name(tx: &Transaction<'_>, user: &Value, floor: i64) -> Result<bool, Error> {
    if !name(tx, 0, user, 0, 0).await? {
        return Ok(false);
    }
    query("UPDATE stats_name_baselines SET refresh_floor=max(coalesce(refresh_floor,?),?) WHERE user_id=?")
        .bind(floor)
        .bind(floor)
        .bind(integer(user, "id")?)
        .execute(&**tx)
        .await?;
    Ok(true)
}
#[expect(clippy::too_many_arguments)]
async fn member(
    tx: &Transaction<'_>,
    bot: i64,
    chat: i64,
    uid: i64,
    present: bool,
    status: Option<&str>,
    date: i64,
    update: i64,
) -> Result<(), Error> {
    if uid <= 0 {
        return Err(Error::Invalid);
    }
    if query("SELECT 1 FROM stats_roster_floors WHERE chat_id=? AND event_floor>?")
        .bind(chat)
        .bind(date)
        .fetch_optional(&**tx)
        .await?
        .is_some()
    {
        return Ok(());
    }
    if !watermark(tx, bot, "member", chat, uid, 0, date, update).await? {
        return Ok(());
    }
    query("INSERT INTO stats_group_members(chat_id,user_id,in_chat,status,synced_at) VALUES(?,?,?,?,?) ON CONFLICT(chat_id,user_id) DO UPDATE SET in_chat=excluded.in_chat,status=excluded.status,synced_at=excluded.synced_at")
        .bind(chat).bind(uid).bind(present).bind(status).bind(date).execute(&**tx).await?;
    query("DELETE FROM stats_chat_admins WHERE chat_id=? AND user_id=?")
        .bind(chat)
        .bind(uid)
        .execute(&**tx)
        .await?;
    if present && matches!(status, Some("administrator" | "creator")) {
        query("INSERT INTO stats_chat_admins(user_id,chat_id,status,synced_at) VALUES(?,?,?,?)")
            .bind(uid)
            .bind(chat)
            .bind(status)
            .bind(date)
            .execute(&**tx)
            .await?;
    }
    Ok(())
}
impl Stats {
    /// Runtime schedules and drains this future. Network failures never erase known names.
    pub async fn refresh_usernames(
        &self,
        timeout: std::time::Duration,
    ) -> Result<crate::NameRefreshReport, Error> {
        if timeout.is_zero() || timeout > std::time::Duration::from_secs(300) {
            return Err(Error::Invalid);
        }
        let users=query("SELECT DISTINCT m.from_user FROM stats_messages m JOIN groups g ON g.id=m.chat_id WHERE g.enabled=1 AND m.from_user IS NOT NULL ORDER BY m.from_user").fetch_all(&self.0.db).await?;
        let groups=query("SELECT DISTINCT m.chat_id FROM stats_messages m JOIN groups g ON g.id=m.chat_id WHERE g.enabled=1 AND m.chat_id<0 ORDER BY m.chat_id").fetch_all(&self.0.db).await?.into_iter().map(|r|r.try_get::<i64,_>(0)).collect::<Result<Vec<_>,_>>()?;
        let mut report = crate::NameRefreshReport::default();
        for row in users {
            let uid = row.try_get::<i64, _>(0)?;
            let started = revision(&self.0.db).await?;
            let floor = seconds(chrono::Utc::now().timestamp())?;
            let mut found = None;
            for chat in groups
                .iter()
                .copied()
                .map(Some)
                .chain(std::iter::once(None))
            {
                if let Ok(Ok(Some(profile))) =
                    tokio::time::timeout(timeout, self.0.membership.lookup_name(chat, uid)).await
                    && profile.user_id == uid
                    && (profile.username.is_some() || profile.display_name.is_some())
                    && profile
                        .username
                        .as_ref()
                        .is_none_or(|s| s.chars().count() <= 64 && !s.chars().any(char::is_control))
                    && profile.display_name.as_ref().is_none_or(|s| {
                        s.chars().count() <= 256 && !s.chars().any(char::is_control)
                    })
                {
                    found = Some(profile);
                    break;
                }
            }
            if let Some(profile) = found {
                let mut conn = self.0.db.connection().await?;
                let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
                    .await
                    .map_err(|_| Error::Database)?;
                if unchanged(&*tx, "name", 0, uid, started).await?
                    && refreshed_name(&tx,&serde_json::json!({"id":uid,"username":profile.username,"first_name":profile.display_name}),floor).await?
                {
                    report.refreshed += 1;
                } else {
                    report.skipped += 1;
                }
                tx.commit().await.map_err(|_| Error::Database)?;
            } else {
                report.skipped += 1;
            }
        }
        Ok(report)
    }
    /// Commit the complete update before the caller advances its durable polling offset.
    /// Unsupported/private updates are ignored; malformed supported updates fail without partial writes.
    pub async fn observe_raw_update(&self, bot_id: i64, raw: Value) -> Result<(), Error> {
        if bot_id <= 0 {
            return Err(Error::Invalid);
        }
        let (kind, value) = [
            "message",
            "edited_message",
            "message_reaction",
            "chat_member",
            "my_chat_member",
        ]
        .into_iter()
        .find_map(|k| raw.get(k).map(|v| (k, v)))
        .unwrap_or(("", &Value::Null));
        if kind.is_empty() {
            return Ok(());
        }
        if kind == "message"
            && value["entities"].as_array().is_some_and(|entities| {
                entities
                    .iter()
                    .any(|e| e["type"] == "bot_command" && e["offset"] == 0)
            })
        {
            return Ok(());
        }
        if !matches!(value["chat"]["type"].as_str(), Some("group" | "supergroup")) {
            return Ok(());
        }
        let chat = integer(&value["chat"], "id")?;
        if chat >= 0 {
            return Err(Error::Invalid);
        }
        let update = integer(&raw, "update_id")?;
        let date = seconds(integer(value, "date")?)?;
        if update < 0 || date < 0 {
            return Err(Error::Invalid);
        }
        let mut conn = self.0.db.connection().await?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        query("INSERT INTO groups(id,title,enabled,created_at) VALUES(?,?,1,?) ON CONFLICT(id) DO NOTHING")
            .bind(chat).bind(text(&value["chat"],"title")).bind(date).execute(&*tx).await?;
        let enabled: bool = query("SELECT enabled FROM groups WHERE id=?")
            .bind(chat)
            .fetch_one(&*tx)
            .await?
            .try_get(0)?;
        if !enabled {
            tx.commit().await.map_err(|_| Error::Database)?;
            return Ok(());
        }
        match kind {
            "message" | "edited_message" => {
                let id = integer(value, "message_id")?;
                if id <= 0 {
                    return Err(Error::Invalid);
                }
                let edited = kind == "edited_message";
                let event_date = if edited {
                    seconds(integer(value, "edit_date")?)?
                } else {
                    date
                };
                if watermark(&tx, bot_id, "message", chat, id, 0, event_date, update).await? {
                    let msg_type = [
                        "sticker",
                        "photo",
                        "video",
                        "animation",
                        "voice",
                        "audio",
                        "document",
                        "video_note",
                        "poll",
                        "location",
                        "game",
                        "new_chat_members",
                        "left_chat_member",
                        "new_chat_title",
                        "new_chat_photo",
                        "pinned_message",
                    ]
                    .into_iter()
                    .find(|k| value.get(*k).is_some_and(|v| !v.is_null()))
                    .unwrap_or("text");
                    let file = if msg_type == "photo" {
                        value["photo"]
                            .as_array()
                            .and_then(|v| v.last())
                            .and_then(|v| text(v, "file_id"))
                    } else {
                        text(&value[msg_type], "file_id")
                    };
                    query("INSERT INTO stats_messages(message_id,chat_id,date,from_user,reply_to_message,forward_from,forward_from_chat,text,caption,msg_type,sticker_set_name,new_chat_title,file_id,is_edited,sender_tag,message_thread_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(chat_id,message_id) DO UPDATE SET text=excluded.text,caption=excluded.caption,msg_type=excluded.msg_type,file_id=excluded.file_id,is_edited=excluded.is_edited,sender_tag=excluded.sender_tag WHERE excluded.is_edited=1 OR stats_messages.is_edited=0")
                        .bind(id).bind(chat).bind(date).bind(value["from"]["id"].as_i64()).bind(value["reply_to_message"]["message_id"].as_i64())
                        .bind(value["forward_origin"]["sender_user"]["id"].as_i64()).bind(value["forward_origin"]["sender_chat"]["id"].as_i64().or(value["forward_origin"]["chat"]["id"].as_i64()))
                        .bind(text(value,"text")).bind(text(value,"caption")).bind(msg_type).bind(text(&value["sticker"],"set_name")).bind(text(value,"new_chat_title")).bind(file).bind(edited).bind(text(value,"sender_tag"))
                        .bind(if value["is_topic_message"].as_bool()==Some(true) { value["message_thread_id"].as_i64() } else { None }).execute(&*tx).await?;
                    if value["from"].is_object() {
                        name(&tx, bot_id, &value["from"], event_date, update).await?;
                    }
                    if !edited {
                        let joined = value["new_chat_members"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        let left = value.get("left_chat_member").filter(|v| v.is_object());
                        for (user, event) in joined
                            .iter()
                            .map(|v| (v, "joined"))
                            .chain(left.map(|v| (v, "left")))
                        {
                            let uid = integer(user, "id")?;
                            query("INSERT INTO stats_user_events(message_id,chat_id,user_id,date,event) SELECT ?,?,?,?,? WHERE NOT EXISTS(SELECT 1 FROM stats_user_events WHERE message_id=? AND chat_id=? AND user_id=? AND event=?)")
                                .bind(id).bind(chat).bind(uid).bind(date).bind(event).bind(id).bind(chat).bind(uid).bind(event).execute(&*tx).await?;
                            member(
                                &tx,
                                bot_id,
                                chat,
                                uid,
                                event == "joined",
                                Some(if event == "joined" { "member" } else { "left" }),
                                date,
                                update,
                            )
                            .await?;
                            name(&tx, bot_id, user, date, update).await?;
                        }
                    }
                }
            }
            "message_reaction" => {
                if value["user"].is_object() {
                    let uid = integer(&value["user"], "id")?;
                    let id = integer(value, "message_id")?;
                    if watermark(&tx, bot_id, "reaction", chat, id, uid, date, update).await? {
                        let mut keys = std::collections::BTreeSet::new();
                        for r in value["new_reaction"].as_array().ok_or(Error::Invalid)? {
                            let rt = text(r, "type").ok_or(Error::Invalid)?;
                            let key = match rt {
                                "emoji" => text(r, "emoji"),
                                "custom_emoji" => text(r, "custom_emoji_id"),
                                "paid" => Some("paid"),
                                _ => None,
                            };
                            if let Some(key) = key {
                                keys.insert(key.to_owned());
                                query("INSERT INTO stats_reactions(user_id,chat_id,message_id,reaction_key,reaction_type,date) VALUES(?,?,?,?,?,?) ON CONFLICT(user_id,chat_id,message_id,reaction_key) DO NOTHING")
                                    .bind(uid).bind(chat).bind(id).bind(key).bind(rt).bind(date).execute(&*tx).await?;
                            }
                        }
                        let stored=query("SELECT reaction_key FROM stats_reactions WHERE chat_id=? AND message_id=? AND user_id=?").bind(chat).bind(id).bind(uid).fetch_all(&*tx).await?;
                        for row in stored {
                            let key = row.try_get::<String, _>(0)?;
                            if !keys.contains(&key) {
                                query("DELETE FROM stats_reactions WHERE chat_id=? AND message_id=? AND user_id=? AND reaction_key=?").bind(chat).bind(id).bind(uid).bind(key).execute(&*tx).await?;
                            }
                        }
                        name(&tx, bot_id, &value["user"], date, update).await?;
                    }
                }
            }
            _ => {
                let m = &value["new_chat_member"];
                let status = text(m, "status").ok_or(Error::Invalid)?;
                let present = match status {
                    "left" | "kicked" => false,
                    "restricted" => m["is_member"].as_bool().unwrap_or(false),
                    _ => true,
                };
                member(
                    &tx,
                    bot_id,
                    chat,
                    integer(&m["user"], "id")?,
                    present,
                    Some(status),
                    date,
                    update,
                )
                .await?;
                name(&tx, bot_id, &m["user"], date, update).await?;
            }
        }
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(())
    }
    pub async fn sync_members(&self, user: i64, chat: i64) -> Result<Value, Error> {
        self.authorize(user, chat, true, false).await?;
        let session = self.0.session.as_ref().ok_or(Error::Unavailable)?;
        let started = revision(&self.0.db).await?;
        let floor = seconds(chrono::Utc::now().timestamp())?;
        let mut roster = std::collections::BTreeMap::new();
        let mut complete = false;
        for page in 0..500_u64 {
            let batch = session
                .get_chat_members(
                    chat,
                    yoink_user_session::ChatMemberFilter::Members,
                    page * 200,
                    200,
                )
                .await?;
            complete = batch.len() < 200;
            for member in batch {
                roster.insert(member.user.id, member);
            }
            if complete {
                break;
            }
        }
        if !complete {
            return Err(Error::Invalid);
        }
        let access = self.authorize(user, chat, true, false).await?;
        let now = chrono::Utc::now().timestamp_micros();
        let mut conn = self.0.db.connection().await?;
        self.authorize_on(&mut conn, user, chat, true, false, access)
            .await?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        if query(
            "SELECT 1 FROM stats_refresh_fences WHERE kind='roster' AND chat_id=? AND revision>?",
        )
        .bind(chat)
        .bind(started)
        .fetch_optional(&*tx)
        .await?
        .is_some()
        {
            tx.commit().await.map_err(|_| Error::Database)?;
            drop(conn);
            return self
                .analytics(
                    user,
                    "members",
                    crate::analytics::Params {
                        chat_id: chat,
                        ..Default::default()
                    },
                )
                .await;
        }
        let changed=query("SELECT subject_id FROM stats_subject_revisions WHERE kind='member' AND chat_id=? AND revision>?").bind(chat).bind(started).fetch_all(&*tx).await?.into_iter().map(|r|r.try_get::<i64,_>(0)).collect::<Result<std::collections::BTreeSet<_>,_>>()?;
        query("DELETE FROM stats_chat_admins WHERE chat_id=? AND NOT EXISTS(SELECT 1 FROM stats_subject_revisions r WHERE r.kind='member' AND r.chat_id=stats_chat_admins.chat_id AND r.subject_id=stats_chat_admins.user_id AND r.revision>?)")
            .bind(chat)
            .bind(started)
            .execute(&*tx)
            .await?;
        query("UPDATE stats_group_members SET in_chat=0,status=NULL,synced_at=? WHERE chat_id=? AND NOT EXISTS(SELECT 1 FROM stats_subject_revisions r WHERE r.kind='member' AND r.chat_id=stats_group_members.chat_id AND r.subject_id=stats_group_members.user_id AND r.revision>?)").bind(now).bind(chat).bind(started).execute(&*tx).await?;
        query("INSERT INTO stats_group_members(chat_id,user_id,in_chat,synced_at) SELECT DISTINCT chat_id,from_user,0,? FROM stats_messages WHERE chat_id=? AND from_user IS NOT NULL ON CONFLICT(chat_id,user_id) DO NOTHING").bind(now).bind(chat).execute(&*tx).await?;
        for (_, m) in roster {
            let uid = m.user.id;
            if changed.contains(&uid) {
                continue;
            }
            query("INSERT INTO stats_group_members(chat_id,user_id,in_chat,status,joined_date,synced_at) VALUES(?,?,1,?,?,?) ON CONFLICT(chat_id,user_id) DO UPDATE SET in_chat=1,status=excluded.status,joined_date=excluded.joined_date,synced_at=excluded.synced_at")
                .bind(chat).bind(uid).bind(&m.status).bind(m.joined_date.map(seconds).transpose()?).bind(now).execute(&*tx).await?;
            if matches!(m.status.as_str(), "administrator" | "creator") {
                query("INSERT INTO stats_chat_admins VALUES(?,?,?,?) ON CONFLICT(user_id,chat_id) DO UPDATE SET status=excluded.status,synced_at=excluded.synced_at")
                    .bind(uid)
                    .bind(chat)
                    .bind(&m.status)
                    .bind(now)
                    .execute(&*tx)
                    .await?;
            }
            if unchanged(&*tx, "name", 0, uid, started).await? {
                refreshed_name(&tx,&serde_json::json!({"id":uid,"username":m.user.username,"first_name":m.user.first_name,"last_name":m.user.last_name}),floor).await?;
            }
        }
        query("UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1")
            .execute(&*tx)
            .await?;
        query("INSERT INTO stats_roster_floors VALUES(?,?) ON CONFLICT(chat_id) DO UPDATE SET event_floor=max(stats_roster_floors.event_floor,excluded.event_floor)").bind(chat).bind(floor).execute(&*tx).await?;
        query("INSERT INTO stats_refresh_fences SELECT 'roster',?,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id) DO UPDATE SET revision=excluded.revision").bind(chat).execute(&*tx).await?;
        tx.commit().await.map_err(|_| Error::Database)?;
        drop(conn);
        self.analytics(
            user,
            "members",
            crate::analytics::Params {
                chat_id: chat,
                ..Default::default()
            },
        )
        .await
    }
}
