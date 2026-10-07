//! Inbox command application facade. Shared dispatcher owns Telegram identity,
//! localized rendering, uncertain send ledger and actual Bot API calls.
use crate::{
    Error, Result,
    contracts::Authorization,
    feed::Filters,
    store::{IngestResult, Store},
};
use serde_json::Value;
/// Arguments take precedence over replied-to text/caption. Never execute crawls
/// inline in a bot update; transactionally enqueue instead.
pub fn extract_url(args: &str, reply: Option<&str>) -> Option<String> {
    fn first(text: &str) -> Option<String> {
        let lower = text.to_ascii_lowercase();
        let start = [lower.find("http://"), lower.find("https://")]
            .into_iter()
            .flatten()
            .min()?;
        let tail = &text[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || [')', ']', '>', '"', '\''].contains(&c))
            .unwrap_or(tail.len());
        Some(tail[..end].to_owned())
    }
    first(args).or_else(|| reply.and_then(first))
}
pub async fn save(
    store: &Store,
    auth: &dyn Authorization,
    user: i64,
    args: &str,
    reply: Option<&str>,
    now: i64,
) -> Result<IngestResult> {
    auth.require(user, "inbox:ingest").await?;
    let url = extract_url(args, reply).ok_or(Error::Invalid)?;
    store.ingest(user, &url, "bot", now).await
}
pub async fn inbox(
    store: &Store,
    auth: &dyn Authorization,
    user: i64,
    limit: usize,
) -> Result<Value> {
    auth.require(user, "inbox:ingest").await?;
    store
        .feed(
            user,
            &Filters {
                limit: Some(limit),
                ..Default::default()
            },
        )
        .await
}
pub async fn stars_sync(
    store: &Store,
    auth: &dyn Authorization,
    user: i64,
    now: i64,
) -> Result<()> {
    auth.require(user, "inbox:gh_sync").await?;
    store.enqueue(user, None, "stars_sync", now).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_reply() {
        assert_eq!(
            extract_url("", Some("look https://example.com/a")),
            Some("https://example.com/a".into())
        );
        assert_eq!(
            extract_url("https://a.com", Some("https://b.com")),
            Some("https://a.com".into())
        );
    }
}

/// Coordinator-only notification address, never included in public item DTOs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyTarget {
    pub chat_id: i64,
    pub message_id: i64,
}
/// Call only after the shared send ledger confirms a Bot API message. First
/// confirmed destination wins; replay cannot retarget another user's item or
/// overwrite an earlier save's placeholder. Uncertain sends must not call this.
pub async fn bind_reply(
    store: &Store,
    auth: &dyn Authorization,
    user: i64,
    item: i64,
    target: ReplyTarget,
) -> Result<bool> {
    auth.require(user, "inbox:ingest").await?;
    if target.chat_id == 0 || target.message_id <= 0 {
        return Err(Error::Invalid);
    }
    use yoink_db::Executor;
    let c = store.db.connection().await?;
    c.batch("BEGIN IMMEDIATE").await?;
    let result=async {
        let rows = yoink_db::query("UPDATE inbox_items SET tg_chat_id=?,tg_reply_message_id=? WHERE user_id=? AND id=? AND source='bot' AND ((tg_chat_id IS NULL AND tg_reply_message_id IS NULL) OR (tg_chat_id=? AND tg_reply_message_id=?))")
            .bind(target.chat_id).bind(target.message_id).bind(user).bind(item).bind(target.chat_id).bind(target.message_id).execute(&*c).await?.rows_affected();
        if rows==1 {yoink_db::query("UPDATE inbox_notifications SET chat_id=?,message_id=?,state='ready' WHERE user_id=? AND item_id=? AND state='waiting'").bind(target.chat_id).bind(target.message_id).bind(user).bind(item).execute(&*c).await?;}
        Ok(rows==1)
    }.await;
    match result {
        Ok(v) => {
            c.batch("COMMIT").await?;
            Ok(v)
        }
        Err(e) => {
            c.batch("ROLLBACK").await?;
            Err(e)
        }
    }
}
/// Re-read after a committed classification, including current authorization.
/// Coordinator owns durable notification admission, localization and delivery.
pub async fn reply_target(
    store: &Store,
    auth: &dyn Authorization,
    user: i64,
    item: i64,
) -> Result<Option<ReplyTarget>> {
    auth.require(user, "inbox:ingest").await?;
    let row = yoink_db::query("SELECT tg_chat_id,tg_reply_message_id FROM inbox_items WHERE user_id=? AND id=? AND status NOT IN ('archived','failed')")
        .bind(user).bind(item).fetch_optional(&store.db).await?;
    let Some(row) = row else { return Ok(None) };
    let chat: Option<i64> = row.try_get("tg_chat_id")?;
    let message: Option<i64> = row.try_get("tg_reply_message_id")?;
    Ok(chat.zip(message).map(|(chat_id, message_id)| ReplyTarget {
        chat_id,
        message_id,
    }))
}
