use std::time::Duration;

use serde_json::{Value, json};
use turso::transaction::{Transaction, TransactionBehavior};
use uuid::Uuid;
use yoink_db::query;
use yoink_domain::{CachedMedia, CachedMediaKind, DownloadParams, DownloadRequest, MediaMode};

use crate::{Store, StoreError, cache_key::media_cache_key, model::Claim};

const MIN_TTL_SECONDS: u64 = 1;
const MAX_TTL_SECONDS: u64 = 31_536_000;
const MAX_ITEMS_JSON_BYTES: usize = 512_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedBatch {
    pub revision: Uuid,
    pub media: Vec<CachedMedia>,
}

impl Store {
    /// Atomically advances the actor/bot message watermark and clears matching URL caches.
    /// A stale message is a successful no-op, including after a cache is repopulated.
    pub async fn clear_media_cache(
        &self,
        actor_id: u64,
        bot_id: i64,
        message_id: i32,
        canonical_source_url: &str,
        configured_owner: bool,
    ) -> Result<Option<u64>, StoreError> {
        let actor_id = i64::try_from(actor_id).map_err(|_| StoreError::InvalidInput)?;
        if !configured_owner {
            return Err(StoreError::Unauthorized);
        }
        if actor_id <= 0
            || bot_id <= 0
            || message_id <= 0
            || canonical_source_url.is_empty()
            || canonical_source_url.len() > 4096
        {
            return Err(StoreError::InvalidInput);
        }
        let parsed = DownloadRequest::new(
            actor_id as u64,
            canonical_source_url,
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .map_err(|_| StoreError::InvalidInput)?;
        if parsed.source_url().as_str() != canonical_source_url {
            return Err(StoreError::InvalidInput);
        }
        let mut connection = self
            .database()
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let authorized = query(include_str!("../sql/authorize_media_cache_clear.sql"))
            .bind(actor_id)
            .fetch_optional(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        if authorized.is_none() {
            tx.rollback().await.map_err(|_| StoreError::Database)?;
            return Err(StoreError::Unauthorized);
        }
        let advanced = query(include_str!(
            "../sql/advance_media_cache_clear_watermark.sql"
        ))
        .bind(actor_id)
        .bind(bot_id)
        .bind(message_id)
        .fetch_optional(&*tx)
        .await
        .map_err(|_| StoreError::Database)?;
        if advanced.is_none() {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(None);
        }
        query(include_str!("../sql/clear_media_cache.sql"))
            .bind(actor_id)
            .bind(bot_id)
            .bind(canonical_source_url)
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let row = query(include_str!("../sql/read_media_cache_clear_changes.sql"))
            .fetch_one(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let affected: i64 = row.try_get("affected").map_err(|_| StoreError::Database)?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(Some(
            u64::try_from(affected).map_err(|_| StoreError::Database)?,
        ))
    }

    pub async fn lookup_media_cache(
        &self,
        request: &DownloadRequest,
    ) -> Result<Option<CachedBatch>, StoreError> {
        let target = request.target().ok_or(StoreError::InvalidInput)?;
        let key = media_cache_key(request)?;
        let row = query(include_str!("../sql/lookup_media_cache.sql"))
            .bind(i64::try_from(request.actor_id()).map_err(|_| StoreError::InvalidInput)?)
            .bind(target.bot_id())
            .bind(key.as_str())
            .fetch_optional(self.database())
            .await
            .map_err(|_| StoreError::Database)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let revision = row.try_get("revision").map_err(|_| StoreError::Database)?;
        let items: Value = row.try_get("media").map_err(|_| StoreError::Database)?;
        Ok(Some(CachedBatch {
            revision,
            media: decode_media(items)?,
        }))
    }

    pub async fn publish_media_cache(
        &self,
        claim: &Claim,
        media: &[CachedMedia],
        ttl: Duration,
    ) -> Result<bool, StoreError> {
        validate_claim_cache(claim)?;
        let target = claim.request.target().ok_or(StoreError::InvalidInput)?;
        let actor_id =
            i64::try_from(claim.request.actor_id()).map_err(|_| StoreError::InvalidInput)?;
        let ttl_seconds = ttl.as_secs();
        if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl_seconds)
            || ttl.subsec_nanos() != 0
            || !(1..=100).contains(&media.len())
        {
            return Err(StoreError::InvalidInput);
        }
        let key = media_cache_key(&claim.request)?;
        let items = encode_media(media)?;
        let request_json = crate::codec::encode(&claim.request)?;
        let attempt = i32::try_from(claim.attempt).map_err(|_| StoreError::InvalidInput)?;
        let item_count = i32::try_from(media.len()).map_err(|_| StoreError::InvalidInput)?;
        let revision = Uuid::new_v4();

        let mut connection = self
            .database()
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;

        let eligible = query(include_str!("../sql/publish_media_cache_eligible.sql"))
            .bind(claim.operation_id)
            .bind(attempt)
            .bind(actor_id)
            .bind(target.bot_id())
            .bind(key.as_str())
            .bind(item_count)
            .bind(revision)
            .bind(items.clone())
            .bind(i64::try_from(ttl_seconds).map_err(|_| StoreError::InvalidInput)?)
            .bind(request_json.clone())
            .fetch_optional(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        if eligible.is_none() {
            tx.rollback().await.map_err(|_| StoreError::Database)?;
            return Ok(false);
        }

        let expired = query(include_str!("../sql/publish_media_cache_prune_select.sql"))
            .bind(actor_id)
            .bind(target.bot_id())
            .bind(key.as_str())
            .fetch_all(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        for row in expired {
            let expired_actor: i64 = row.try_get("actor_id").map_err(|_| StoreError::Database)?;
            let expired_bot: i64 = row.try_get("bot_id").map_err(|_| StoreError::Database)?;
            let expired_key: String = row.try_get("cache_key").map_err(|_| StoreError::Database)?;
            query(include_str!("../sql/publish_media_cache_prune_delete.sql"))
                .bind(expired_actor)
                .bind(expired_bot)
                .bind(expired_key)
                .execute(&*tx)
                .await
                .map_err(|_| StoreError::Database)?;
        }

        let published = query(include_str!("../sql/publish_media_cache.sql"))
            .bind(claim.operation_id)
            .bind(attempt)
            .bind(actor_id)
            .bind(target.bot_id())
            .bind(key.as_str())
            .bind(item_count)
            .bind(revision)
            .bind(items)
            .bind(i64::try_from(ttl_seconds).map_err(|_| StoreError::InvalidInput)?)
            .bind(request_json)
            .fetch_optional(&*tx)
            .await
            .map_err(|_| StoreError::Database)?
            .is_some();
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(published)
    }

    /// Drop a known-invalid revision without restarting any operation or touching
    /// a concurrent replacement. Used after a confirmed prefix cannot be replayed.
    pub async fn invalidate_media_cache(
        &self,
        request: &DownloadRequest,
        revision: Uuid,
    ) -> Result<bool, StoreError> {
        let target = request.target().ok_or(StoreError::InvalidInput)?;
        let key = media_cache_key(request)?;
        let mut connection = self
            .database()
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        query(include_str!("../sql/invalidate_media_cache.sql"))
            .bind(i64::try_from(request.actor_id()).map_err(|_| StoreError::InvalidInput)?)
            .bind(target.bot_id())
            .bind(key.as_str())
            .bind(revision)
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let row = query("SELECT changes() AS affected")
            .fetch_one(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let affected: i64 = row.try_get("affected").map_err(|_| StoreError::Database)?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(affected == 1)
    }

    pub async fn requeue_invalid_cache(
        &self,
        claim: &Claim,
        ordinal: u32,
        revision: Uuid,
    ) -> Result<bool, StoreError> {
        validate_claim_cache(claim)?;
        if ordinal != 0 {
            return Ok(false);
        }
        let target = claim.request.target().ok_or(StoreError::InvalidInput)?;
        let key = media_cache_key(&claim.request)?;
        let attempt = i32::try_from(claim.attempt).map_err(|_| StoreError::InvalidInput)?;
        let mut connection = self
            .database()
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;

        query(include_str!("../sql/requeue_invalid_cache_mark.sql"))
            .bind(claim.operation_id)
            .bind(attempt)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .bind(key.as_str())
            .bind(revision)
            .bind(target.bot_id())
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let marked = query("SELECT changes() AS affected")
            .fetch_one(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let affected: i64 = marked
            .try_get("affected")
            .map_err(|_| StoreError::Database)?;
        if affected != 1 {
            tx.rollback().await.map_err(|_| StoreError::Database)?;
            return Ok(false);
        }

        query(include_str!("../sql/requeue_invalid_cache_operation.sql"))
            .bind(claim.operation_id)
            .bind(attempt)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let updated = query("SELECT changes() AS affected")
            .fetch_one(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let affected: i64 = updated
            .try_get("affected")
            .map_err(|_| StoreError::Database)?;
        if affected != 1 {
            tx.rollback().await.map_err(|_| StoreError::Database)?;
            return Ok(false);
        }

        query(include_str!("../sql/requeue_invalid_cache_evict.sql"))
            .bind(claim.operation_id)
            .bind(attempt)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .bind(key.as_str())
            .bind(revision)
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Database)?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(true)
    }
}

fn validate_claim_cache(claim: &Claim) -> Result<(), StoreError> {
    if claim.attempt == 0
        || claim.attempt > i32::MAX as u32
        || claim.worker_id.is_empty()
        || claim.worker_id.len() > 128
        || !claim.worker_id.is_ascii()
        || claim.worker_id.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(StoreError::InvalidInput);
    }
    if claim.request.target().is_none() {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}

fn encode_media(media: &[CachedMedia]) -> Result<Value, StoreError> {
    if !(1..=100).contains(&media.len()) {
        return Err(StoreError::InvalidInput);
    }
    let result: Vec<Value> = media
        .iter()
        .map(|item| {
            json!({
                "kind": match item.kind() {
                    CachedMediaKind::Photo => "photo",
                    CachedMediaKind::Video => "video",
                    CachedMediaKind::Audio => "audio",
                    CachedMediaKind::Document => "document",
                },
                "file_id": item.file_id(),
                "size_bytes": item.size_bytes(),
            })
        })
        .collect();
    let value = Value::Array(result);
    if serde_json::to_vec(&value)
        .map_err(|_| StoreError::InvalidInput)?
        .len()
        > MAX_ITEMS_JSON_BYTES
    {
        return Err(StoreError::InvalidInput);
    }
    Ok(value)
}

fn decode_media(value: Value) -> Result<Vec<CachedMedia>, StoreError> {
    let encoded = serde_json::to_vec(&value).map_err(|_| StoreError::InvalidStoredData)?;
    if encoded.len() > MAX_ITEMS_JSON_BYTES {
        return Err(StoreError::InvalidStoredData);
    }
    let values = value.as_array().ok_or(StoreError::InvalidStoredData)?;
    if !(1..=100).contains(&values.len()) {
        return Err(StoreError::InvalidStoredData);
    }
    values
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or(StoreError::InvalidStoredData)?;
            if object.len() != 3 {
                return Err(StoreError::InvalidStoredData);
            }
            let kind = match object.get("kind").and_then(Value::as_str) {
                Some("photo") => CachedMediaKind::Photo,
                Some("video") => CachedMediaKind::Video,
                Some("audio") => CachedMediaKind::Audio,
                Some("document") => CachedMediaKind::Document,
                _ => return Err(StoreError::InvalidStoredData),
            };
            let file_id = object
                .get("file_id")
                .and_then(Value::as_str)
                .ok_or(StoreError::InvalidStoredData)?;
            let size_bytes = object
                .get("size_bytes")
                .and_then(Value::as_u64)
                .ok_or(StoreError::InvalidStoredData)?;
            CachedMedia::new(kind, file_id.to_owned(), size_bytes)
                .map_err(|_| StoreError::InvalidStoredData)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_redacts_bounded_media_payloads() {
        let media =
            CachedMedia::new(CachedMediaKind::Video, "private-media-id".into(), 42).unwrap();
        let encoded = encode_media(std::slice::from_ref(&media)).unwrap();
        assert_eq!(decode_media(encoded).unwrap(), vec![media]);
        assert!(
            !format!(
                "{:?}",
                decode_media(json!([{
                    "kind": "video", "file_id": "private-media-id", "size_bytes": 42
                }]))
                .unwrap()
            )
            .contains("private-media-id")
        );
        for invalid in [
            json!([]),
            json!([{"kind":"unknown","file_id":"x","size_bytes":1}]),
            json!([{"kind":"photo","file_id":"bad id","size_bytes":1}]),
            json!([{"kind":"photo","file_id":"x","size_bytes":0}]),
            json!([{"kind":"photo","file_id":"x","size_bytes":1,"extra":true}]),
        ] {
            assert_eq!(decode_media(invalid), Err(StoreError::InvalidStoredData));
        }
        assert_eq!(encode_media(&[]), Err(StoreError::InvalidInput));
    }
}
