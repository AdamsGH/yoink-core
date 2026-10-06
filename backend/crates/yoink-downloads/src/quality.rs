use uuid::Uuid;
use yoink_domain::{
    DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest, MediaMode,
};
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
    pub async fn quality_cut(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<crate::CutSession, DownloadError> {
        self.authorize_quality(actor, target)?;
        self.authorize_chat(actor, target).await?;
        self.store
            .quality_cut(actor, target, id, revision)
            .await
            .map_err(map_quality_error)
    }
    pub async fn submit_telegram_request(
        &self,
        request: &DownloadRequest,
        message_id: i32,
        overrides: DownloadPreferenceOverrides,
    ) -> Result<TelegramSubmission, DownloadError> {
        self.submit_telegram_context(request, message_id, overrides, None)
            .await
    }

    pub(crate) fn validate_telegram_request(
        &self,
        request: &DownloadRequest,
        message_id: i32,
        overrides: &DownloadPreferenceOverrides,
    ) -> Result<(), DownloadError> {
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
            crate::profile::resolve_format_values(request.media_mode(), None, overrides)
                .map_err(map_profile)?;
        }
        Ok(())
    }

    pub(crate) async fn submit_telegram_context(
        &self,
        request: &DownloadRequest,
        message_id: i32,
        overrides: DownloadPreferenceOverrides,
        telegram: Option<&yoink_storage::TelegramMediaAdmission>,
    ) -> Result<TelegramSubmission, DownloadError> {
        self.validate_telegram_request(request, message_id, &overrides)?;
        let target = request
            .target()
            .copied()
            .ok_or(DownloadError::InvalidInput)?;
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
            if let Some(context) = telegram {
                let identity = self.authorize_request(request).await?;
                return self
                    .admit_network_context(request, &intent_key, &identity, Some(context))
                    .await
                    .map(TelegramSubmission::Admitted);
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
        if telegram.is_none()
            && let Some(admission) = self
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
                let session = self
                    .store
                    .quality_begin_media(
                        request.actor_id(),
                        target,
                        message_id,
                        &draft,
                        telegram.map(|c| &c.original),
                    )
                    .await
                    .map_err(map_quality_error)?;
                return Ok(TelegramSubmission::Quality(Box::new(session)));
            }
        }
        self.submit_preferred_context(request, &intent_key, overrides, telegram)
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
            let (original, overrides) = selected_request(current.request(), choice)?;
            let key = format!(
                "tg:{}:{}:{}",
                target.bot_id(),
                target.chat_id(),
                current.origin_message_id
            );
            return self
                .submit_quality_media(&original, &key, overrides, id, current.origin_message_id)
                .await;
        }
        let s = self
            .store
            .quality_claim(actor, target, id, revision, choice)
            .await
            .map_err(map_quality_error)?;
        let (original, overrides) = selected_request(s.request(), choice)?;
        let key = format!(
            "tg:{}:{}:{}",
            target.bot_id(),
            target.chat_id(),
            s.origin_message_id
        );
        let admission = self
            .submit_quality_media(&original, &key, overrides, id, s.origin_message_id)
            .await?;
        self.store
            .quality_submitted(actor, target, id, s.revision, admission.operation_id)
            .await
            .map_err(map_quality_error)?;
        Ok(admission)
    }
    pub(crate) async fn submit_quality_media(
        &self,
        request: &DownloadRequest,
        key: &str,
        overrides: DownloadPreferenceOverrides,
        id: Uuid,
        message_id: i32,
    ) -> Result<Admission, DownloadError> {
        if let Some(original) = self
            .store
            .quality_media_intent(id)
            .await
            .map_err(map_quality_error)?
        {
            self.authorize_request(request).await?;
            if let Some(admission) = self
                .store
                .replay_telegram_media(&original, key)
                .await
                .map_err(map_quality_error)?
            {
                return Ok(admission);
            }
            let target = request
                .target()
                .copied()
                .ok_or(DownloadError::InvalidInput)?;
            let expected = self
                .store
                .deferred_media_state(request.actor_id(), target)
                .await
                .map_err(map_quality_error)?;
            let context = yoink_storage::TelegramMediaAdmission {
                original,
                message_id,
                expected,
            };
            self.submit_preferred_context(request, key, overrides, Some(&context))
                .await
        } else {
            self.submit_preferred_request(request, key, overrides).await
        }
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

fn selected_request(
    draft: &DownloadRequest,
    choice: QualityChoice,
) -> Result<(DownloadRequest, DownloadPreferenceOverrides), DownloadError> {
    if choice == QualityChoice::Audio {
        let params = draft.params();
        let audio = DownloadRequest::new(
            draft.actor_id(),
            draft.source_url().as_str(),
            MediaMode::Audio,
            draft.clip_range(),
            DownloadParams::default()
                .with_engine(params.engine())
                .with_playlist(params.playlist())
                .with_clips(params.clips().to_vec())
                .map_err(|_| DownloadError::InvalidInput)?,
        )
        .map_err(|_| DownloadError::InvalidInput)?;
        let mut audio = if let Some(target) = draft.target().copied() {
            audio
                .with_target(target)
                .map_err(|_| DownloadError::InvalidInput)?
        } else {
            return Err(DownloadError::InvalidInput);
        };
        audio = audio
            .with_network(draft.network().clone())
            .map_err(|_| DownloadError::InvalidInput)?;
        let saved = draft.preference_overrides().cloned().unwrap_or_default();
        let overrides = DownloadPreferenceOverrides {
            audio_codec: saved.audio_codec,
            send_as_file: saved.send_as_file,
            ..Default::default()
        };
        Ok((audio, overrides))
    } else {
        let original = without_overrides(draft.clone())?;
        let mut overrides = draft.preference_overrides().cloned().unwrap_or_default();
        if overrides.max_height.is_some() {
            return Err(DownloadError::InvalidPreferences);
        }
        overrides.max_height = Some(choice.max_height());
        Ok((original, overrides))
    }
}

#[cfg(test)]
mod tests {
    use super::{quality_asks, selected_request};
    use yoink_domain::{
        AudioCodec, ClipRange, Container, DeliveryTarget, DownloadParams,
        DownloadPreferenceOverrides, DownloadRequest, Engine, MediaMode, PlaylistRange, VideoCodec,
    };
    use yoink_storage::QualityChoice;

    #[test]
    fn ask_preference_matching_trims_and_ignores_ascii_case() {
        assert!(quality_asks(Some("ask")));
        assert!(quality_asks(Some("  AsK  ")));
        assert!(!quality_asks(Some("best")));
        assert!(!quality_asks(None));
    }

    #[test]
    fn audio_selection_preserves_structure_and_filters_video_overrides() {
        let target = DeliveryTarget::new(12, 42, None, Some(77)).unwrap();
        let draft = DownloadRequest::new(
            42,
            "https://example.com/audio?secret=private",
            MediaMode::Video,
            Some(ClipRange::new(1.0, 4.0).unwrap()),
            DownloadParams::default()
                .with_engine(Engine::YtDlp)
                .with_playlist(Some(PlaylistRange::new(2, Some(4)).unwrap())),
        )
        .unwrap()
        .with_target(target)
        .unwrap()
        .with_preference_overrides(DownloadPreferenceOverrides {
            video_codec: Some(VideoCodec::H265),
            container: Some(Container::Mkv),
            max_height: Some(Some(720)),
            audio_codec: Some(AudioCodec::Aac),
            send_as_file: Some(true),
            ..Default::default()
        });
        let (audio, overrides) = selected_request(&draft, QualityChoice::Audio).unwrap();
        assert_eq!(audio.media_mode(), MediaMode::Audio);
        assert_eq!(audio.source_url(), draft.source_url());
        assert_eq!(audio.actor_id(), draft.actor_id());
        assert_eq!(audio.target(), Some(&target));
        assert_eq!(audio.clip_range(), draft.clip_range());
        assert_eq!(audio.params().engine(), Engine::YtDlp);
        assert_eq!(audio.params().playlist(), draft.params().playlist());
        assert_eq!(audio.params().clips(), draft.params().clips());
        assert_eq!(overrides.audio_codec, Some(AudioCodec::Aac));
        assert_eq!(overrides.send_as_file, Some(true));
        assert_eq!(overrides.max_height, None);
        assert_eq!(overrides.video_codec, None);
        assert_eq!(overrides.container, None);
        assert!(!format!("{audio:?}").contains("secret=private"));
    }
}
