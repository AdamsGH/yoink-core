use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use yoink_executor::ValidatedArtifact;
use yoink_storage::{TextIdentity, TextOutcome};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTarget {
    pub bot_id: i64,
    pub chat_id: i64,
    pub topic_id: Option<i32>,
    pub incoming_message_id: i32,
}
pub enum MusicAudioArtifact {
    Upload(ValidatedArtifact),
    Cached(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioReceipt {
    pub message_id: i32,
    pub file_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioOutcome {
    Confirmed(i32),
    Unknown,
    Disabled,
    Rejected,
}
pub struct AudioCommand<'a> {
    pub actor: MusicActor,
    pub target: AudioTarget,
    pub track: &'a Track,
    pub caption: MusicCard,
}
pub struct AudioDownloadLog {
    pub actor: MusicActor,
    pub target: AudioTarget,
    pub track: Track,
    pub status: &'static str,
    pub error: Option<MusicError>,
    pub size_bytes: Option<u64>,
}
pub trait MusicAudioDelivery: Send + Sync {
    /// Exactly one send_audio call. Timeout/disconnect/lost receipt is unknown,
    /// never definite rejection. Adapters must not detach or retry the request.
    fn send(
        &self,
        target: AudioTarget,
        title: String,
        artist: Option<String>,
        caption: MusicCard,
        artifact: MusicAudioArtifact,
    ) -> MusicFuture<'_, AudioReceipt>;
}
fn digest(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
impl MusicService {
    /// Call only after the card is sent. Immutable replay uses the shared effect
    /// journal, not a second media dispatcher or a text transport.
    pub async fn send_audio(
        &self,
        command: AudioCommand<'_>,
        persistence: Arc<dyn MusicPersistence>,
        delivery: Arc<dyn MusicAudioDelivery>,
        cancel: CancellationToken,
    ) -> Result<AudioOutcome, MusicError> {
        let AudioCommand {
            actor,
            target,
            track,
            caption,
        } = command;
        self.authorize(actor).await?;
        if !self.settings.download_enabled {
            return Ok(AudioOutcome::Disabled);
        }
        if target.bot_id <= 0
            || target.chat_id == 0
            || target.incoming_message_id <= 0
            || target.topic_id.is_some_and(|i| i <= 0)
            || actor.group.map(|g| g.0).unwrap_or(actor.user_id) != target.chat_id
            || actor.group.and_then(|g| g.1) != target.topic_id.map(i64::from)
            || caption.text.encode_utf16().count() > 1024
        {
            return Err(MusicError::InvalidInput);
        }
        let credential_identity = digest(
            &serde_json::to_vec(&self.settings.credentials)
                .map_err(|_| MusicError::InvalidInput)?,
        );
        let payload = serde_json::json!({"kind":"music_audio","bind_item":false,"track":track,"caption":caption,"actor":actor.user_id,"group":actor.group,"target":target,"credential_identity":credential_identity});
        let identity = TextIdentity {
            bot: target.bot_id,
            chat: target.chat_id,
            topic: target.topic_id,
            message: target.incoming_message_id,
            actor: actor.user_id,
            item: None,
            edit: false,
            text: MusicCard::from_resolution(&Resolution::Track(track.clone())).title,
            payload,
        };
        if !identity.valid() {
            return Err(MusicError::InvalidInput);
        }
        let token = format!(
            "music:audio:{}",
            digest(
                format!(
                    "{}:{}:{:?}:{}:{}:{}",
                    target.bot_id,
                    target.chat_id,
                    target.topic_id,
                    target.incoming_message_id,
                    actor.user_id,
                    track.source.url.as_url()
                )
                .as_bytes()
            )
        );
        let budget = self.settings.persistence_timeout;
        if let Some(effect) = self
            .persistence_admission(&cancel, persistence.effect(token.clone()))
            .await?
        {
            if effect.identity.as_ref() != Some(&identity) {
                return Err(MusicError::Conflict);
            }
            return Ok(match effect.outcome {
                TextOutcome::Confirmed(id) => AudioOutcome::Confirmed(id),
                _ => AudioOutcome::Unknown,
            });
        }
        let cache_key = format!(
            "music:native:{}",
            digest(
                &serde_json::to_vec(&(
                    "music-mp3-id3-192k-v1",
                    actor.user_id,
                    target.bot_id,
                    &credential_identity,
                    &track.metadata,
                    &track.source
                ))
                .map_err(|_| MusicError::InvalidInput)?
            )
        );
        let cached = match self
            .persistence_admission(&cancel, persistence.cached(cache_key.clone()))
            .await
        {
            Err(MusicError::Cancelled) => return Err(MusicError::Cancelled),
            result => result.ok().flatten(),
        };
        let artifact = if let Some(id) = cached {
            MusicAudioArtifact::Cached(id)
        } else {
            let artifacts = timeout(
                self.settings.audio_timeout,
                self.audio_sources(track, cancel.clone()),
            )
            .await
            .map_err(|_| MusicError::SourceUnavailable)
            .and_then(|r| r);
            let artifacts = match artifacts {
                Ok(a) => a,
                Err(error) => {
                    self.audio_log(
                        persistence.as_ref(),
                        AudioDownloadLog {
                            actor,
                            target,
                            track: track.clone(),
                            status: "error",
                            error: Some(error),
                            size_bytes: None,
                        },
                    )
                    .await;
                    return Err(error);
                }
            };
            let artifact = artifacts
                .into_iter()
                .next()
                .ok_or(MusicError::SourceUnavailable)?;
            MusicAudioArtifact::Upload(artifact)
        };
        let (status, size_bytes) = match &artifact {
            MusicAudioArtifact::Cached(_) => ("cached", None),
            MusicAudioArtifact::Upload(a) => ("ok", Some(a.size_bytes)),
        };
        self.authorize(actor).await?;
        if cancel.is_cancelled() {
            return Err(MusicError::Cancelled);
        }
        if !self
            .persistence_admission(&cancel, persistence.begin(token.clone(), identity))
            .await?
        {
            return Ok(AudioOutcome::Unknown);
        }
        if let Err(error) = self.authorize(actor).await {
            let _ = timeout(budget, persistence.unknown(token)).await;
            return Err(error);
        }
        // Once armed, cancellation cannot authorize another external attempt.
        let sent = tokio::select! {_=cancel.cancelled()=>Err(MusicError::DeliveryUnknown),r=timeout(self.settings.delivery_timeout,delivery.send(target,track.metadata.title.clone(),track.metadata.artist.clone(),caption,artifact))=>r.unwrap_or(Err(MusicError::DeliveryUnknown))};
        match sent {
            Ok(receipt) if receipt.message_id > 0 => {
                // A positive receipt remains authoritative even if DB/cache is down.
                let _ = timeout(budget, persistence.confirm(token, receipt.message_id)).await;
                self.audio_log(
                    persistence.as_ref(),
                    AudioDownloadLog {
                        actor,
                        target,
                        track: track.clone(),
                        status,
                        error: None,
                        size_bytes,
                    },
                )
                .await;
                if let Some(id) = receipt.file_id {
                    let _ = timeout(
                        budget,
                        persistence.cache(
                            cache_key,
                            id,
                            MusicCard::from_resolution(&Resolution::Track(track.clone())).title,
                            self.settings.cache_ttl,
                        ),
                    )
                    .await;
                }
                Ok(AudioOutcome::Confirmed(receipt.message_id))
            }
            Err(MusicError::DeliveryRejected) => {
                let _ = timeout(budget, persistence.unknown(token)).await;
                Ok(AudioOutcome::Rejected)
            }
            _ => {
                let _ = timeout(budget, persistence.unknown(token)).await;
                Ok(AudioOutcome::Unknown)
            }
        }
    }
    async fn audio_log(&self, persistence: &dyn MusicPersistence, log: AudioDownloadLog) {
        if !matches!(
            timeout(
                self.settings.persistence_timeout,
                persistence.audio_log(log)
            )
            .await,
            Ok(Ok(()))
        ) {
            tracing::warn!("music audio analytics persistence failed");
        }
    }
    async fn audio_sources(
        &self,
        track: &Track,
        cancel: CancellationToken,
    ) -> Result<Vec<ValidatedArtifact>, MusicError> {
        let mut candidates: Vec<_> = track
            .links
            .iter()
            .filter(|l| l.provider == MusicProvider::Ytmusic)
            .chain(
                track
                    .links
                    .iter()
                    .filter(|l| l.provider == MusicProvider::Youtube),
            )
            .map(|l| l.url.clone())
            .collect();
        let query = track
            .metadata
            .artist
            .as_ref()
            .map(|a| format!("{a} {}", track.metadata.title))
            .unwrap_or_else(|| track.metadata.title.clone());
        let mut searches = [MusicProvider::Ytmusic, MusicProvider::Youtube].into_iter();
        let mut index = 0;
        loop {
            if index >= candidates.len() {
                let Some(provider) = searches.next() else {
                    return Err(MusicError::SourceUnavailable);
                };
                if let Ok(MusicResult::Link { url: Some(url) }) = self
                    .lookup(
                        MusicTask::MusicSearch {
                            provider,
                            query: query.clone(),
                            title: track.metadata.title.clone(),
                            artist: track.metadata.artist.clone().unwrap_or_default(),
                        },
                        provider,
                        cancel.clone(),
                    )
                    .await
                    && parse_link(url.as_url().as_str()).is_ok_and(|l| {
                        matches!(l.provider, MusicProvider::Ytmusic | MusicProvider::Youtube)
                    })
                    && !candidates.contains(&url)
                {
                    candidates.push(url);
                }
                if index >= candidates.len() {
                    continue;
                }
            }
            let url = candidates[index].clone();
            let provider = parse_link(url.as_url().as_str())?.provider;
            index += 1;
            if !parse_link(url.as_url().as_str()).is_ok_and(|l| {
                matches!(l.provider, MusicProvider::Ytmusic | MusicProvider::Youtube)
            }) {
                continue;
            }
            let task = MusicTask::MusicAudio {
                url,
                title: track.metadata.title.clone(),
                artist: track.metadata.artist.clone().unwrap_or_default(),
                thumbnail_url: track.metadata.thumbnail_url.clone(),
            };
            let result = tokio::select! {_=cancel.cancelled()=>return Err(MusicError::Cancelled),r=async {let _slot=self.slots.acquire().await.map_err(|_|MusicError::Cancelled)?;self.worker.audio(self.settings.request(task,provider),cancel.clone(),self.settings.audio_timeout,self.settings.audio_max_bytes).await}=>r};
            match result {
                Ok(a) if a.len() == 1 && a[0].media_type == "audio/mpeg" => return Ok(a),
                Err(e @ (MusicError::ResourceLimit | MusicError::Cancelled)) => {
                    if !matches!(e, MusicError::Cancelled) {
                        tracing::warn!(provider = provider.key(), error = ?e, "Music audio attempt failed");
                    }
                    return Err(e);
                }
                Err(e) => {
                    tracing::warn!(provider = provider.key(), error = ?e, "Music audio attempt failed")
                }
                _ => {}
            }
        }
    }
}
