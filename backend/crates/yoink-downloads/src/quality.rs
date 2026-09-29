use uuid::Uuid;
use yoink_domain::{DeliveryTarget, DownloadPreferenceOverrides, DownloadRequest, MediaMode};
use yoink_storage::{Admission, StoreError};

use crate::{DownloadError, DownloadService, map_profile, map_store};

pub use yoink_storage::{QualityChoice, QualitySession, QualityStage};

pub(crate) fn quality_asks(value: Option<&str>) -> bool {
    value.is_some_and(|quality| quality.trim().eq_ignore_ascii_case("ask"))
}

#[derive(Clone, Debug, PartialEq)]
pub enum TelegramSubmission {
    Admitted(Admission),
    Quality(Box<QualitySession>),
}

fn map_quality_error(error: StoreError) -> DownloadError {
    match error {
        StoreError::NotFoundOrStale => DownloadError::Conflict,
        other => map_store(other),
    }
}

impl DownloadService {
    fn authorize_quality(&self, actor: u64, target: DeliveryTarget) -> Result<(), DownloadError> {
        if !self
            .identities
            .is_configured_owner(i64::try_from(actor).map_err(|_| DownloadError::Denied)?)
            || target.chat_id() <= 0
            || target.chat_id() as u64 != actor
            || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        Ok(())
    }
    pub async fn quality_current(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<Option<QualitySession>, DownloadError> {
        self.authorize_quality(actor, target)?;
        self.authorize_chat(actor, target).await?;
        self.store
            .quality_current(actor, target)
            .await
            .map_err(map_quality_error)
    }
    pub async fn submit_telegram_request(
        &self,
        request: &DownloadRequest,
        message_id: i32,
        overrides: DownloadPreferenceOverrides,
    ) -> Result<TelegramSubmission, DownloadError> {
        if message_id <= 0 {
            return Err(DownloadError::InvalidInput);
        }
        let target = request
            .target()
            .copied()
            .ok_or(DownloadError::InvalidInput)?;
        if target
            .reply_to_message_id()
            .is_some_and(|id| id != message_id)
            || target.bot_id() != self.delivery.bot_id()
        {
            return Err(DownloadError::InvalidInput);
        }
        if !crate::request_format_defaults(request)
            || request.preference_overrides().is_some()
            || request.network() != &yoink_domain::NetworkSelection::NONE
            || request.source_url().as_str().len() > 4096
        {
            return Err(DownloadError::InvalidInput);
        }
        if request.media_mode() != MediaMode::Formats {
            crate::profile::resolve_format_values(request.media_mode(), None, &overrides)
                .map_err(map_profile)?;
        }
        let intent_key = format!("tg:{}:{}:{}", target.bot_id(), target.chat_id(), message_id);
        self.authorize_request(request).await?;
        let intent = request.clone().with_preference_overrides(overrides.clone());
        let private_owner = self
            .identities
            .is_configured_owner(request.actor_id() as i64)
            && target.chat_id() == request.actor_id() as i64
            && target.thread_id().is_none();
        if request.media_mode() == MediaMode::Formats {
            if !private_owner || overrides != DownloadPreferenceOverrides::default() {
                return Err(DownloadError::Denied);
            }
            return self
                .submit_request(request, &intent_key)
                .await
                .map(TelegramSubmission::Admitted);
        }
        let private_owner_video = private_owner && request.media_mode() == MediaMode::Video;
        // Quality state exists only for the owner-private interactive flow. Group submissions
        // retain the ordinary admission path and never query private quality state.
        if private_owner
            && let Some(current) = self
                .store
                .quality_current(request.actor_id(), target)
                .await
                .map_err(map_quality_error)?
        {
            if message_id < current.origin_message_id {
                return Err(DownloadError::Conflict);
            }
            if current.origin_message_id == message_id {
                if current.request() != &intent {
                    return Err(DownloadError::Conflict);
                }
                return Ok(TelegramSubmission::Quality(Box::new(current)));
            }
            if current.stage == QualityStage::Confirming {
                return Err(DownloadError::Conflict);
            }
        }
        if let Some(admission) = self
            .store
            .replay_network_admission(&intent, &intent_key)
            .await
            .map_err(map_quality_error)?
        {
            return Ok(TelegramSubmission::Admitted(admission));
        }
        if private_owner_video && overrides.max_height.is_none() {
            let mut check = overrides.clone();
            check.max_height = Some(None);
            let saved = self
                .profile
                .format_preferences(request.actor_id())
                .await
                .map_err(map_profile)?;
            if saved
                .as_ref()
                .and_then(|p| p.quality.as_deref())
                .is_some_and(|q| quality_asks(Some(q)))
            {
                let mut saved = saved.ok_or(DownloadError::InvalidPreferences)?;
                saved.quality = Some("best".into());
                crate::profile::resolve_format_values(request.media_mode(), Some(&saved), &check)
                    .map_err(map_profile)?;
                let draft = request.clone().with_preference_overrides(overrides);
                return self
                    .store
                    .quality_begin(request.actor_id(), target, message_id, &draft)
                    .await
                    .map(|session| TelegramSubmission::Quality(Box::new(session)))
                    .map_err(map_quality_error);
            }
        }
        self.submit_preferred_request(request, &intent_key, overrides)
            .await
            .map(TelegramSubmission::Admitted)
    }
    pub async fn quality_confirm(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        choice: QualityChoice,
    ) -> Result<Admission, DownloadError> {
        self.authorize_quality(actor, target)?;
        self.authorize_chat(actor, target).await?;
        if QualityChoice::parse(choice.as_str()).is_err() {
            return Err(DownloadError::InvalidInput);
        }
        if let Some(current) = self
            .store
            .quality_current(actor, target)
            .await
            .map_err(map_quality_error)?
            && current.id == id
            && current.stage == QualityStage::Submitted
        {
            if revision != current.revision - 2
                && revision != current.revision - 1
                && revision != current.revision
            {
                return Err(DownloadError::Conflict);
            }
            if current.selected != Some(choice) {
                return Err(DownloadError::Conflict);
            }
            let original = without_overrides(current.request().clone())?;
            let mut overrides = current
                .request()
                .preference_overrides()
                .cloned()
                .unwrap_or_default();
            overrides.max_height = Some(choice.max_height());
            let key = format!(
                "tg:{}:{}:{}",
                target.bot_id(),
                target.chat_id(),
                current.origin_message_id
            );
            return self
                .submit_preferred_request(&original, &key, overrides)
                .await;
        }
        let s = self
            .store
            .quality_claim(actor, target, id, revision, choice)
            .await
            .map_err(map_quality_error)?;
        let original = without_overrides(s.request().clone())?;
        let mut overrides = s
            .request()
            .preference_overrides()
            .cloned()
            .unwrap_or_default();
        if overrides.max_height.is_some() {
            return Err(DownloadError::InvalidPreferences);
        }
        overrides.max_height = Some(choice.max_height());
        let key = format!(
            "tg:{}:{}:{}",
            target.bot_id(),
            target.chat_id(),
            s.origin_message_id
        );
        let admission = self
            .submit_preferred_request(&original, &key, overrides)
            .await?;
        self.store
            .quality_submitted(actor, target, id, s.revision, admission.operation_id)
            .await
            .map_err(map_quality_error)?;
        Ok(admission)
    }
    pub async fn quality_cancel(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<QualitySession, DownloadError> {
        self.authorize_quality(actor, target)?;
        self.authorize_chat(actor, target).await?;
        self.store
            .quality_cancel(actor, target, id, revision)
            .await
            .map_err(map_quality_error)
    }
}

fn without_overrides(request: DownloadRequest) -> Result<DownloadRequest, DownloadError> {
    let mut original = DownloadRequest::new(
        request.actor_id(),
        request.source_url().as_str(),
        request.media_mode(),
        request.clip_range(),
        request.params().clone(),
    )
    .map_err(|_| DownloadError::InvalidInput)?;
    if let Some(target) = request.target().copied() {
        original = original
            .with_target(target)
            .map_err(|_| DownloadError::InvalidInput)?;
    }
    original
        .with_network(request.network().clone())
        .map_err(|_| DownloadError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::quality_asks;

    #[test]
    fn ask_preference_matching_trims_and_ignores_ascii_case() {
        assert!(quality_asks(Some("ask")));
        assert!(quality_asks(Some("  AsK  ")));
        assert!(!quality_asks(Some("best")));
        assert!(!quality_asks(None));
    }
}
