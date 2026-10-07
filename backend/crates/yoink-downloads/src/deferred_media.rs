use crate::{DownloadError, DownloadService, TelegramSubmission, map_store};
use yoink_domain::{DeliveryTarget, DownloadPreferenceOverrides, DownloadRequest, MediaMode};
use yoink_storage::TelegramMediaAdmission;

impl DownloadService {
    pub async fn arm_media(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        mode: MediaMode,
    ) -> Result<(), DownloadError> {
        if target.bot_id() != self.delivery.bot_id()
            || target.chat_id() != actor as i64
            || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        // Use the exact eventual download authorization (feature, current role/ban).
        let probe = DownloadRequest::new(
            actor,
            "https://example.com/",
            mode,
            None,
            yoink_domain::DownloadParams::default(),
        )
        .and_then(|r| r.with_target(target))
        .map_err(|_| DownloadError::InvalidInput)?;
        self.authorize_request(&probe).await?;
        self.store
            .arm_deferred_media(
                actor,
                target,
                message_id,
                mode,
                self.identities.is_configured_owner(actor as i64),
            )
            .await
            .map_err(map_store)
    }

    /// Called only by the native Telegram adapter. `plain` is not a public API field.
    pub async fn submit_telegram_media(
        &self,
        request: &DownloadRequest,
        message_id: i32,
        overrides: DownloadPreferenceOverrides,
        plain: bool,
    ) -> Result<TelegramSubmission, DownloadError> {
        let target = request
            .target()
            .copied()
            .ok_or(DownloadError::InvalidInput)?;
        if target.chat_id() != request.actor_id() as i64 || target.thread_id().is_some() {
            return self
                .submit_telegram_request(request, message_id, overrides)
                .await;
        }
        self.validate_telegram_request(request, message_id, &overrides)?;
        self.authorize_request(request).await?;
        let original = if matches!(
            request.media_mode(),
            MediaMode::Formats | MediaMode::Links | MediaMode::AudioLinks
        ) {
            request.clone()
        } else {
            request.clone().with_preference_overrides(overrides.clone())
        };
        let key = format!("tg:{}:{}:{}", target.bot_id(), target.chat_id(), message_id);
        if let Some(admission) = self
            .store
            .replay_telegram_media(&original, &key)
            .await
            .map_err(map_store)?
        {
            return Ok(TelegramSubmission::Admitted(admission));
        }
        // Optimistic preparation; the Immediate admission transaction verifies the
        // state version before charging/inserting. Races retry with fresh preferences
        // and network authority, never with a stale resolved mode.
        for _ in 0..8 {
            let expected = self
                .store
                .deferred_media_state(request.actor_id(), target)
                .await
                .map_err(map_store)?;
            let mode = if plain {
                expected
                    .as_ref()
                    .filter(|s| s.message_id < message_id)
                    .and_then(|s| s.mode)
                    .unwrap_or(request.media_mode())
            } else {
                request.media_mode()
            };
            let resolved = DownloadRequest::new(
                request.actor_id(),
                request.source_url().as_str(),
                mode,
                request.clip_range(),
                request.params().clone(),
            )
            .and_then(|r| r.with_target(target))
            .map_err(|_| DownloadError::InvalidInput)?;
            let context = TelegramMediaAdmission {
                original: original.clone(),
                message_id,
                expected,
            };
            match self
                .submit_telegram_context(&resolved, message_id, overrides.clone(), Some(&context))
                .await
            {
                Err(DownloadError::Conflict) => {
                    if let Some(admission) = self
                        .store
                        .replay_telegram_media(&original, &key)
                        .await
                        .map_err(map_store)?
                    {
                        return Ok(TelegramSubmission::Admitted(admission));
                    }
                    let current = self
                        .store
                        .deferred_media_state(request.actor_id(), target)
                        .await
                        .map_err(map_store)?;
                    if current == context.expected {
                        return Err(DownloadError::Conflict);
                    }
                }
                result => return result,
            }
        }
        Err(DownloadError::Conflict)
    }
}
