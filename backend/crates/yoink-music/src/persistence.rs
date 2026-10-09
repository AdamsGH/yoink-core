use crate::*;
use std::sync::Arc;
use yoink_db::{Database, query};
use yoink_storage::{Store, TextIdentity, TextOutcome};
#[derive(Clone)]
pub struct NativeMusicPersistence {
    pub database: Database,
    pub store: Arc<Store>,
}
impl ResolveAnalytics for NativeMusicPersistence {
    fn record(&self, actor: i64, track: Track) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            query(include_str!("../sql/resolve_log.sql"))
                .bind(actor)
                .bind(track.source.provider.key())
                .bind(
                    track
                        .metadata
                        .artist
                        .as_ref()
                        .map(|a| a.chars().take(256).collect::<String>()),
                )
                .bind(track.metadata.title.chars().take(256).collect::<String>())
                .bind(track.links.len() as i64)
                .execute(&self.database)
                .await
                .map_err(|_| MusicError::Persistence)?;
            Ok(())
        })
    }
}
pub trait MusicPersistence: Send + Sync {
    fn audio_log(&self, log: AudioDownloadLog) -> MusicFuture<'_, ()>;
    fn effect(&self, token: String) -> MusicFuture<'_, Option<AudioEffect>>;
    fn begin(&self, token: String, identity: TextIdentity) -> MusicFuture<'_, bool>;
    fn confirm(&self, token: String, message: i32) -> MusicFuture<'_, ()>;
    fn unknown(&self, token: String) -> MusicFuture<'_, ()>;
    fn cached(&self, key: String) -> MusicFuture<'_, Option<String>>;
    fn cache(
        &self,
        key: String,
        file_id: String,
        title: String,
        ttl: std::time::Duration,
    ) -> MusicFuture<'_, ()>;
}
#[derive(Debug, Clone)]
pub struct AudioEffect {
    pub identity: Option<TextIdentity>,
    pub outcome: TextOutcome,
}
impl MusicPersistence for NativeMusicPersistence {
    fn audio_log(&self, log: AudioDownloadLog) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            query(include_str!("../sql/audio_log.sql"))
                .bind(log.actor.user_id)
                .bind(log.track.source.url.as_url().as_str())
                .bind(log.track.source.url.as_url().host_str())
                .bind(log.track.metadata.title)
                .bind(log.status)
                .bind(log.error.map(|e| e.to_string()))
                .bind(log.actor.group.map(|g| g.0))
                .bind(log.target.topic_id)
                .bind(log.target.incoming_message_id)
                .bind(log.size_bytes.and_then(|b| i64::try_from(b).ok()))
                .execute(&self.database)
                .await
                .map_err(|_| MusicError::Persistence)?;
            Ok(())
        })
    }
    fn effect(&self, token: String) -> MusicFuture<'_, Option<AudioEffect>> {
        Box::pin(async move {
            self.store
                .text_effect(&token)
                .await
                .map_err(|_| MusicError::Persistence)
                .map(|e| {
                    e.map(|e| AudioEffect {
                        identity: e.identity,
                        outcome: e.outcome,
                    })
                })
        })
    }
    fn begin(&self, token: String, identity: TextIdentity) -> MusicFuture<'_, bool> {
        Box::pin(async move {
            self.store.begin_text(&token, &identity).await.map_err(|e| {
                if e == yoink_storage::StoreError::Conflict {
                    MusicError::Conflict
                } else {
                    MusicError::Persistence
                }
            })
        })
    }
    fn confirm(&self, token: String, message: i32) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            self.store
                .confirm_text(&token, message)
                .await
                .map_err(|_| MusicError::Persistence)?;
            self.store
                .mark_text_bound(&token)
                .await
                .map_err(|_| MusicError::Persistence)
        })
    }
    fn unknown(&self, token: String) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            self.store
                .unknown_text(&token)
                .await
                .map_err(|_| MusicError::Persistence)
        })
    }
    fn cached(&self, key: String) -> MusicFuture<'_, Option<String>> {
        Box::pin(async move {
            let r = query(include_str!("../sql/cache_get.sql"))
                .bind(key)
                .fetch_optional(&self.database)
                .await
                .map_err(|_| MusicError::Persistence)?;
            r.map(|r| r.try_get("file_id").map_err(|_| MusicError::Persistence))
                .transpose()
        })
    }
    fn cache(
        &self,
        key: String,
        file_id: String,
        title: String,
        ttl: std::time::Duration,
    ) -> MusicFuture<'_, ()> {
        Box::pin(async move {
            if file_id.is_empty() || file_id.len() > 256 {
                return Err(MusicError::InvalidInput);
            }
            query(include_str!("../sql/cache_put.sql"))
                .bind(key)
                .bind(file_id)
                .bind(title)
                .bind(ttl.as_micros() as i64)
                .execute(&self.database)
                .await
                .map_err(|_| MusicError::Persistence)?;
            Ok(())
        })
    }
}
