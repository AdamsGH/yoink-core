use crate::{Error, Stats};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use turso::transaction::TransactionBehavior;
use yoink_access::Role;
use yoink_db::{Transaction, query};

#[derive(Clone, Copy, Debug, Default)]
pub struct ImportCounts {
    pub inserted: u64,
    pub skipped: u64,
    pub events: u64,
    pub total: u64,
}
fn user(v: &Value) -> Option<i64> {
    v.as_str()
        .and_then(|s| s.strip_prefix("user"))
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_i64())
        .filter(|id| *id > 0)
}
fn flatten(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_owned();
    }
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().or_else(|| v["text"].as_str()))
                .collect()
        })
        .unwrap_or_default()
}
fn date(v: &Value) -> Result<i64, Error> {
    let s = v["date"].as_str().ok_or(Error::Invalid)?;
    let d = DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").map(|d| d.and_utc()))
        .map_err(|_| Error::Invalid)?;
    if d.timestamp() < 0 {
        return Err(Error::Invalid);
    }
    Ok(d.timestamp_micros())
}
fn bounded(s: Option<&str>, max: usize) -> Result<Option<&str>, Error> {
    if s.is_some_and(|s| s.chars().count() > max) {
        Err(Error::Invalid)
    } else {
        Ok(s)
    }
}
impl Stats {
    /// Atomic additive import. Existing live rows, names and core users are untouched.
    pub async fn import_desktop(
        &self,
        actor: i64,
        chat: i64,
        bytes: Vec<u8>,
        max_messages: usize,
    ) -> Result<ImportCounts, Error> {
        #[cfg(test)]
        {
            self.desktop_inner(
                actor,
                chat,
                bytes,
                max_messages,
                (&self.0.search_tasks, None),
                None,
            )
            .await
        }
        #[cfg(not(test))]
        {
            self.desktop_inner(
                actor,
                chat,
                bytes,
                max_messages,
                (&self.0.search_tasks, None),
            )
            .await
        }
    }
    pub(super) async fn desktop_inner(
        &self,
        actor: i64,
        chat: i64,
        bytes: Vec<u8>,
        max_messages: usize,
        ownership: (
            &tokio_util::task::TaskTracker,
            Option<std::sync::Arc<super::ImportInput>>,
        ),
        #[cfg(test)] pause: Option<(&tokio::sync::Notify, &tokio::sync::Notify)>,
    ) -> Result<ImportCounts, Error> {
        if self.role(actor).await? != Role::Owner {
            return Err(Error::Forbidden);
        }
        if chat >= 0
            || max_messages == 0
            || max_messages > 100_000
            || bytes.len() > 16 * 1024 * 1024
        {
            return Err(Error::Invalid);
        }
        let (tasks, input) = ownership;
        // Drop an undelivered parsed export before its capacity reservation.
        let (messages, _input) = yoink_stats_search::tracked_blocking(tasks, move || {
            Ok((super::parser::messages(&bytes, max_messages), input))
        })
        .await
        .map_err(|_| Error::Unavailable)?;
        let messages = messages?;
        let mut conn = self.0.db.connection().await?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        self.import_owner(&tx, actor).await?;
        let mut counts = ImportCounts {
            total: messages.len() as u64,
            ..Default::default()
        };
        for m in &messages {
            tokio::task::yield_now().await;
            let Some(id) = m["id"].as_i64().filter(|id| *id > 0) else {
                counts.skipped += 1;
                continue;
            };
            if !matches!(m["type"].as_str(), Some("message" | "service")) {
                counts.skipped += 1;
                continue;
            }
            let timestamp = date(m)?;
            let service = m["type"] == "service";
            let uid = user(&m[if service { "actor_id" } else { "from_id" }]);
            let content = flatten(&m["text"]);
            let action = m["action"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("service");
            let media = m["media_type"].as_str();
            let kind = if service {
                match action {
                    "edit_group_title" => "new_chat_title",
                    "pin_message" => "pinned_message",
                    "edit_group_photo" => "new_chat_photo",
                    "invite_members" | "join_group_by_link" => "new_chat_members",
                    "remove_members" => "left_chat_member",
                    s => s,
                }
            } else if m["photo"].as_str().is_some() {
                "photo"
            } else if let Some(media) = media {
                match media {
                    "audio_file" => "audio",
                    "video_file" => "video",
                    "voice_message" => "voice",
                    "video_message" => "video_note",
                    s => s,
                }
            } else if !content.is_empty() {
                "text"
            } else if m["poll"].is_object() {
                "poll"
            } else {
                counts.skipped += 1;
                continue;
            };
            bounded(Some(kind), 32)?;
            let title = bounded(m["title"].as_str(), 256)?;
            let sticker = bounded(m["sticker_emoji"].as_str(), 128)?;
            let file = if media == Some("sticker") {
                m["file"].as_str().filter(|s| !s.contains(".webp"))
            } else {
                None
            };
            bounded(file, 256)?;
            let is_media = kind != "text" && !service;
            let inserted = query("INSERT INTO stats_messages(message_id,chat_id,date,from_user,reply_to_message,forward_from,text,caption,msg_type,sticker_set_name,new_chat_title,file_id,is_edited) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,0) ON CONFLICT(chat_id,message_id) DO NOTHING")
                .bind(id).bind(chat).bind(timestamp).bind(uid).bind(m["reply_to_message_id"].as_i64())
                .bind(if m["forwarded_from"].as_str().is_some() { uid } else { None })
                .bind(if !is_media && !content.is_empty() { Some(content.as_str()) } else { None })
                .bind(if is_media && !content.is_empty() { Some(content.as_str()) } else { None })
                .bind(kind).bind(sticker).bind(title).bind(file).execute(&*tx).await?.rows_affected();
            if inserted == 0 {
                counts.skipped += 1;
                continue;
            }
            counts.inserted += 1;
            if let Some(uid) = uid
                && let Some(name) = bounded(m["from"].as_str(), 256)?
            {
                query("INSERT INTO stats_user_names(user_id,date,display_name) SELECT ?,?,? WHERE NOT EXISTS(SELECT 1 FROM stats_user_names WHERE user_id=?)")
                        .bind(uid).bind(Utc::now().timestamp_micros()).bind(name).bind(uid).execute(&*tx).await?;
            }
            if service
                && matches!(
                    action,
                    "invite_members" | "join_group_by_link" | "remove_members"
                )
            {
                let mut members: Vec<i64> = m["members"]
                    .as_array()
                    .map(|a| a.iter().filter_map(user).collect())
                    .unwrap_or_default();
                if members.is_empty() && action != "remove_members" {
                    members.extend(uid);
                }
                members.sort_unstable();
                members.dedup();
                for member in members {
                    tokio::task::yield_now().await;
                    let added = query("INSERT INTO stats_user_events(message_id,chat_id,user_id,date,event) SELECT ?,?,?,?,? WHERE NOT EXISTS(SELECT 1 FROM stats_user_events WHERE message_id=? AND chat_id=? AND user_id=? AND event=?)")
                        .bind(id).bind(chat).bind(member).bind(timestamp).bind(if action == "remove_members" { "left" } else { "joined" })
                        .bind(id).bind(chat).bind(member).bind(if action == "remove_members" { "left" } else { "joined" }).execute(&*tx).await?;
                    counts.events += added.rows_affected();
                }
            }
            #[cfg(test)]
            if counts.inserted == 1
                && let Some((entered, release)) = pause
            {
                entered.notify_one();
                release.notified().await;
            }
        }
        self.import_owner(&tx, actor).await?;
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(counts)
    }
    async fn import_owner(&self, conn: &turso::Connection, actor: i64) -> Result<(), Error> {
        let snapshot = self
            .0
            .identity
            .snapshot_in_transaction(conn, actor, None)
            .await
            .map_err(|_| Error::Database)?
            .ok_or(Error::Forbidden)?;
        if snapshot.principal.role != Role::Owner || snapshot.principal.is_blocked(Utc::now()) {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
