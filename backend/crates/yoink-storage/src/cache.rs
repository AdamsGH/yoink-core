use std::time::Duration;

use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;
use yoink_domain::{CachedMedia, CachedMediaKind, DownloadRequest};

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
    pub async fn lookup_media_cache(
        &self,
        request: &DownloadRequest,
    ) -> Result<Option<CachedBatch>, StoreError> {
        let target = request.target().ok_or(StoreError::InvalidInput)?;
        let key = media_cache_key(request)?;
        let row = sqlx::query(include_str!("../sql/lookup_media_cache.sql"))
            .bind(i64::try_from(request.actor_id()).map_err(|_| StoreError::InvalidInput)?)
            .bind(target.bot_id())
            .bind(key.as_str())
            .fetch_optional(self.pool())
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
        let row = sqlx::query(include_str!("../sql/publish_media_cache.sql"))
            .bind(claim.operation_id)
            .bind(i32::try_from(claim.attempt).map_err(|_| StoreError::InvalidInput)?)
            .bind(actor_id)
            .bind(target.bot_id())
            .bind(key.as_str())
            .bind(media.len() as i32)
            .bind(Uuid::new_v4())
            .bind(items)
            .bind(ttl_seconds as i64)
            .bind(request_json)
            .fetch_one(self.pool())
            .await
            .map_err(|_| StoreError::Database)?;
        row.try_get("published").map_err(|_| StoreError::Database)
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
        let result = sqlx::query(include_str!("../sql/invalidate_media_cache.sql"))
            .bind(i64::try_from(request.actor_id()).map_err(|_| StoreError::InvalidInput)?)
            .bind(target.bot_id())
            .bind(key.as_str())
            .bind(revision)
            .execute(self.pool())
            .await
            .map_err(|_| StoreError::Database)?;
        Ok(result.rows_affected() == 1)
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
        let row = sqlx::query(include_str!("../sql/requeue_invalid_cache.sql"))
            .bind(claim.operation_id)
            .bind(i32::try_from(claim.attempt).map_err(|_| StoreError::InvalidInput)?)
            .bind(claim.lease_token)
            .bind(&claim.worker_id)
            .bind(key.as_str())
            .bind(revision)
            .bind(target.bot_id())
            .fetch_one(self.pool())
            .await
            .map_err(|_| StoreError::Database)?;
        row.try_get("requeued").map_err(|_| StoreError::Database)
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
