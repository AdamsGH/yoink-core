//! Transactional completion outbox. Claimed work is repeatable only before the
//! durable send boundary. Sending/unknown work needs an explicit delivery-ledger
//! outcome; it is never put back on the ordinary retry queue after a crash.
use crate::{
    Error, Result,
    contracts::{Authorization, Call},
    store::{Job, Store},
    telegram::ReplyTarget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yoink_db::{Executor, query};

/// Shared runtime must re-evaluate current destination/group/topic/private-route
/// authority. This supplements, never substitutes for, current user grants/bans.
pub trait RouteAuthority: Send + Sync {
    fn require<'a>(&'a self, user: i64, target: ReplyTarget) -> Call<'a, ()>;
}
/// Persisted model; payload is an immutable classification snapshot, not a token.
#[derive(Clone, Serialize, Deserialize)]
pub struct Notification {
    pub sequence: i64,
    pub id: String,
    pub user_id: i64,
    pub item_id: Option<i64>,
    pub completion_key: String,
    pub payload: Value,
    pub chat_id: Option<i64>,
    pub message_id: Option<i64>,
    pub state: String,
    pub attempts: i64,
    pub available_at: i64,
    pub lease_until: Option<i64>,
    pub lease_token: Option<String>,
    pub send_token: Option<String>,
    pub receipt: Option<String>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
impl std::fmt::Debug for Notification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notification")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}
fn decode(r: yoink_db::Row) -> Result<Notification> {
    Ok(Notification {
        sequence: r.try_get("sequence")?,
        id: r.try_get("id")?,
        user_id: r.try_get("user_id")?,
        item_id: r.try_get("item_id")?,
        completion_key: r.try_get("completion_key")?,
        payload: r.try_get("payload")?,
        chat_id: r.try_get("chat_id")?,
        message_id: r.try_get("message_id")?,
        state: r.try_get("state")?,
        attempts: r.try_get("attempts")?,
        available_at: r.try_get("available_at")?,
        lease_until: r.try_get("lease_until")?,
        lease_token: r.try_get("lease_token")?,
        send_token: r.try_get("send_token")?,
        receipt: r.try_get("receipt")?,
        error_code: r.try_get("error_code")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}
/// Called inside apply_classification's transaction, after rules, before job done.
pub(crate) async fn admit<E: Executor>(
    c: &E,
    job: &Job,
    categories: &[String],
    now: i64,
) -> Result<()> {
    let item = job.item_id.ok_or(Error::Invalid)?;
    let Some(r)=query("SELECT title,url,summary,status,source,tg_chat_id,tg_reply_message_id FROM inbox_items WHERE id=? AND user_id=?").bind(item).bind(job.user_id).fetch_optional(c).await? else {return Ok(())};
    let status: String = r.try_get("status")?;
    if r.try_get::<String, _>("source")? != "bot"
        || ["archived", "failed"].contains(&status.as_str())
    {
        return Ok(());
    }
    let payload = json!({"title":r.try_get::<Option<String>,_>("title")?,"url":r.try_get::<String,_>("url")?,"summary":r.try_get::<Option<String>,_>("summary")?,"categories":categories});
    let chat: Option<i64> = r.try_get("tg_chat_id")?;
    let message: Option<i64> = r.try_get("tg_reply_message_id")?;
    let bound = chat.is_some_and(|c| c != 0) && message.is_some_and(|m| m > 0);
    query("INSERT INTO inbox_notifications(id,user_id,item_id,completion_key,payload,chat_id,message_id,state,available_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(completion_key) DO NOTHING")
        .bind(uuid::Uuid::new_v4().to_string()).bind(job.user_id).bind(item).bind(format!("{}:{}",job.id,job.lease_token)).bind(payload).bind(if bound{chat}else{None}).bind(if bound{message}else{None}).bind(if bound{"ready"}else{"waiting"}).bind(now).bind(now).bind(now).execute(c).await?;
    Ok(())
}
impl Store {
    pub async fn notification(&self, user: i64, id: &str) -> Result<Notification> {
        decode(
            query("SELECT * FROM inbox_notifications WHERE user_id=? AND id=?")
                .bind(user)
                .bind(id)
                .fetch_optional(&self.db)
                .await?
                .ok_or(Error::NotFound)?,
        )
    }
    /// Claim transaction also quarantines crashed sends and cancels inactive
    /// pre-send events. It never clears send_token or leases unknown work again.
    pub async fn claim_notification(&self, now: i64, lease: i64) -> Result<Option<Notification>> {
        let until = now
            .checked_add(lease)
            .filter(|_| lease > 0)
            .ok_or(Error::Invalid)?;
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
            query("UPDATE inbox_notifications SET state='unknown',error_code='outcome_unknown',updated_at=? WHERE state='sending' AND lease_until<=?").bind(now).bind(now).execute(&*c).await?;
            query("UPDATE inbox_notifications SET state='cancelled',error_code='inactive',updated_at=? WHERE state IN ('waiting','ready','claimed') AND NOT EXISTS(SELECT 1 FROM inbox_items i WHERE i.id=inbox_notifications.item_id AND i.user_id=inbox_notifications.user_id AND i.status NOT IN ('archived','failed'))").bind(now).execute(&*c).await?;
            query("UPDATE inbox_notifications SET state='claimed',attempts=attempts+1,lease_until=?,lease_token=?,updated_at=? WHERE id=(SELECT n.id FROM inbox_notifications n WHERE ((n.state='ready' AND n.available_at<=?) OR (n.state='claimed' AND n.lease_until<=?)) AND NOT EXISTS(SELECT 1 FROM inbox_notifications prior WHERE prior.chat_id=n.chat_id AND prior.message_id=n.message_id AND prior.state IN ('ready','claimed','sending','unknown') AND prior.sequence<n.sequence) ORDER BY n.sequence LIMIT 1) RETURNING *").bind(until).bind(uuid::Uuid::new_v4().to_string()).bind(now).bind(now).bind(now).fetch_optional(&*c).await?.map(decode).transpose()
        }.await;
        match result {
            Ok(r) => {
                c.batch("COMMIT").await?;
                Ok(r)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
    /// Fresh grant and route checks immediately before CAS send admission. The
    /// resulting send_token is the shared send-ledger idempotency identity.
    pub async fn begin_notification_send(
        &self,
        auth: &dyn Authorization,
        route: &dyn RouteAuthority,
        n: &Notification,
        clock: impl Fn() -> i64,
    ) -> Result<String> {
        auth.require(n.user_id, "inbox:ingest").await?;
        let target = ReplyTarget {
            chat_id: n.chat_id.ok_or(Error::Invalid)?,
            message_id: n.message_id.ok_or(Error::Invalid)?,
        };
        route.require(n.user_id, target).await?;
        auth.require(n.user_id, "inbox:ingest").await?;
        let now = clock();
        let token = uuid::Uuid::new_v4().to_string();
        let changed=query("UPDATE inbox_notifications SET state='sending',send_token=?,updated_at=? WHERE id=? AND user_id=? AND state='claimed' AND lease_token=? AND lease_until>? AND chat_id=? AND message_id=? AND EXISTS(SELECT 1 FROM inbox_items i WHERE i.id=inbox_notifications.item_id AND i.user_id=inbox_notifications.user_id AND i.status NOT IN ('archived','failed') AND i.tg_chat_id=inbox_notifications.chat_id AND i.tg_reply_message_id=inbox_notifications.message_id)")
            .bind(&token).bind(now).bind(&n.id).bind(n.user_id).bind(&n.lease_token).bind(now).bind(target.chat_id).bind(target.message_id).execute(&self.db).await?.rows_affected();
        if changed == 1 {
            Ok(token)
        } else {
            Err(Error::Uncertain)
        }
    }
    /// Pre-send retry is fenced by the live claim. It cannot retry sending work.
    pub async fn retry_notification(
        &self,
        n: &Notification,
        now: i64,
        available_at: i64,
    ) -> Result<bool> {
        if available_at < now {
            return Err(Error::Invalid);
        }
        Ok(query("UPDATE inbox_notifications SET state='ready',available_at=?,lease_token=NULL,lease_until=NULL,updated_at=? WHERE id=? AND user_id=? AND state='claimed' AND lease_token=? AND lease_until>?").bind(available_at).bind(now).bind(&n.id).bind(n.user_id).bind(&n.lease_token).bind(now).execute(&self.db).await?.rows_affected()==1)
    }
    /// Record a delivery-ledger result, including after claim expiry/restart.
    /// No authorization is needed to record an already confirmed effect; user
    /// and immutable send_token are still fenced. This NEVER initiates a send.
    pub async fn notification_outcome(
        &self,
        user: i64,
        id: &str,
        send_token: &str,
        outcome: Outcome,
        now: i64,
    ) -> Result<bool> {
        let (state, receipt, error, available) = match outcome {
            Outcome::Confirmed { receipt } => {
                if receipt.is_empty() || receipt.len() > 128 {
                    return Err(Error::Invalid);
                }
                ("acked", Some(receipt), None, now)
            }
            Outcome::Unknown => ("unknown", None, Some("outcome_unknown"), now),
            Outcome::DefinitelyNotSent { retry_at } => {
                if retry_at < now {
                    return Err(Error::Invalid);
                }
                ("ready", None, Some("not_sent"), retry_at)
            }
        };
        Ok(query("UPDATE inbox_notifications SET state=?,receipt=?,error_code=?,available_at=?,lease_token=NULL,lease_until=NULL,updated_at=? WHERE user_id=? AND id=? AND send_token=? AND state IN ('sending','unknown')")
            .bind(state).bind(receipt).bind(error).bind(available).bind(now).bind(user).bind(id).bind(send_token).execute(&self.db).await?.rows_affected()==1)
    }
    pub async fn unknown_notifications(&self, user: i64) -> Result<Vec<Notification>> {
        query("SELECT * FROM inbox_notifications WHERE user_id=? AND state IN ('sending','unknown') ORDER BY sequence").bind(user).fetch_all(&self.db).await?.into_iter().map(decode).collect()
    }
}
/// Only an authoritative shared ledger/transport result may assert not-sent.
/// Timeouts, connection resets, missing receipts and HTTP 5xx are Unknown.
pub enum Outcome {
    Confirmed { receipt: String },
    DefinitelyNotSent { retry_at: i64 },
    Unknown,
}
