//! Trusted Telegram admission context, never serialized into worker or cache metadata.
use crate::{Store, StoreError};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;
use yoink_domain::{DeliveryTarget, DownloadRequest, MediaMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredMediaState {
    pub message_id: i32,
    pub mode: Option<MediaMode>,
}
#[derive(Clone, Debug)]
pub struct TelegramMediaAdmission {
    pub original: DownloadRequest,
    pub message_id: i32,
    pub expected: Option<DeferredMediaState>,
}

pub(crate) fn mode_text(mode: MediaMode) -> Result<&'static str, StoreError> {
    match mode {
        MediaMode::Video => Ok("video"),
        MediaMode::Audio => Ok("audio"),
        MediaMode::Image => Ok("image"),
        _ => Err(StoreError::InvalidInput),
    }
}
pub(crate) async fn read_state(
    db: &impl yoink_db::Executor,
    actor: u64,
    target: DeliveryTarget,
) -> Result<Option<DeferredMediaState>, StoreError> {
    let row = query(include_str!("../sql/deferred_media_read.sql"))
        .bind(actor as i64)
        .bind(target.bot_id())
        .bind(target.chat_id())
        .fetch_optional(db)
        .await
        .map_err(|_| StoreError::Database)?;
    row.map(|row| {
        let mode: Option<String> = row.try_get("mode").map_err(|_| StoreError::Database)?;
        Ok(DeferredMediaState {
            message_id: row
                .try_get("message_id")
                .map_err(|_| StoreError::Database)?,
            mode: match mode.as_deref() {
                None => None,
                Some("video") => Some(MediaMode::Video),
                Some("audio") => Some(MediaMode::Audio),
                Some("image") => Some(MediaMode::Image),
                _ => return Err(StoreError::Database),
            },
        })
    })
    .transpose()
}
impl Store {
    pub async fn quality_media_intent(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<DownloadRequest>, StoreError> {
        query(include_str!("../sql/quality_media_intent_read.sql"))
            .bind(id)
            .fetch_optional(&self.database)
            .await
            .map_err(|_| StoreError::Database)?
            .map(|r| crate::codec::decode(r.try_get("intent").map_err(|_| StoreError::Database)?))
            .transpose()
    }
    pub async fn deferred_media_state(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<Option<DeferredMediaState>, StoreError> {
        let connection = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        read_state(&*connection, actor, target).await
    }
    /// Role/ban rechecked inside the same mutation transaction. Feature authorization
    /// is additionally performed by DownloadService before entering this boundary.
    pub async fn arm_deferred_media(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        mode: MediaMode,
        configured_owner: bool,
    ) -> Result<(), StoreError> {
        if target.chat_id() != actor as i64
            || target.chat_id() <= 0
            || target.thread_id().is_some()
            || message_id <= 0
        {
            return Err(StoreError::InvalidInput);
        }
        let mode = mode_text(mode)?;
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        if query(include_str!("../sql/deferred_media_authorize.sql"))
            .bind(actor as i64)
            .bind(chrono::Utc::now())
            .bind(configured_owner)
            .fetch_optional(&*tx)
            .await
            .map_err(|_| StoreError::Database)?
            .is_none()
        {
            return Err(StoreError::Unauthorized);
        }
        query(include_str!("../sql/deferred_media_arm.sql"))
            .bind(actor as i64)
            .bind(target.bot_id())
            .bind(target.chat_id())
            .bind(message_id)
            .bind(mode)
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        tx.commit().await.map_err(|_| StoreError::Database)
    }
}
