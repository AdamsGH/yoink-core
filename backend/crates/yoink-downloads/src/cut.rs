//! Persistent clip conversations. Transport adapters never own the admission recipe.
use uuid::Uuid;
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_storage::Admission;

pub use yoink_storage::QualityChoice;
pub use yoink_storage::{CutSession, CutStage};

#[derive(Clone, Debug, PartialEq)]
pub enum CutSubmission {
    Admitted(Admission),
    Quality(Box<CutSession>),
}

use crate::{DownloadError, DownloadService, map_store};

impl DownloadService {
    async fn authorize_cut(&self, actor: u64, target: DeliveryTarget) -> Result<(), DownloadError> {
        if !self
            .identities
            .is_configured_owner(i64::try_from(actor).map_err(|_| DownloadError::Denied)?)
            || target.chat_id() <= 0
            || target.chat_id() as u64 != actor
            || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        self.authorize_chat(actor, target).await
    }

    pub async fn cut_current(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<Option<CutSession>, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_current(actor, target)
            .await
            .map_err(map_store)
    }

    pub async fn cut_begin(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        url: &str,
        start: Option<f64>,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_begin(actor, target, message_id, url, start)
            .await
            .map_err(map_store)
    }

    pub async fn cut_input(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        value: f64,
        numeric: bool,
    ) -> Result<Option<CutSession>, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_input(actor, target, message_id, value, numeric)
            .await
            .map_err(map_store)
    }

    pub async fn cut_edit(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_edit(actor, target, id, revision)
            .await
            .map_err(map_store)
    }

    pub async fn cut_add(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_add(actor, target, id, revision)
            .await
            .map_err(map_store)
    }

    pub async fn cut_edit_segment(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        index: usize,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_edit_segment(actor, target, id, revision, index)
            .await
            .map_err(map_store)
    }

    pub async fn cut_delete_segment(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        index: usize,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_delete_segment(actor, target, id, revision, index)
            .await
            .map_err(map_store)
    }

    pub async fn cut_back(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_back(actor, target, id, revision)
            .await
            .map_err(map_store)
    }

    pub async fn cut_cancel(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        message_id: Option<i32>,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        self.store
            .cut_cancel(actor, target, id, revision, message_id)
            .await
            .map_err(map_store)
    }

    async fn cut_quality_required(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<bool, DownloadError> {
        self.authorize_cut(actor, target).await?;
        let saved = self
            .profile
            .format_preferences(actor)
            .await
            .map_err(crate::map_profile)?;
        Ok(crate::quality::quality_asks(
            saved.as_ref().and_then(|p| p.quality.as_deref()),
        ))
    }

    pub async fn cut_choose(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        choice: QualityChoice,
    ) -> Result<CutSession, DownloadError> {
        self.authorize_cut(actor, target).await?;
        if choice == QualityChoice::Audio {
            return Err(DownloadError::InvalidInput);
        }
        self.store
            .cut_choose(actor, target, id, revision, choice)
            .await
            .map_err(map_store)
    }

    /// Moves a Ready clip into its durable chooser for saved `ask`, otherwise admits it with the existing cut key.
    pub async fn cut_submit(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSubmission, DownloadError> {
        self.authorize_cut(actor, target).await?;
        let session = self
            .store
            .cut_current(actor, target)
            .await
            .map_err(map_store)?
            .ok_or(DownloadError::Conflict)?;
        if session.id != id || session.revision != revision {
            return Err(DownloadError::Conflict);
        }
        if session.stage == CutStage::Ready && self.cut_quality_required(actor, target).await? {
            return self
                .store
                .cut_quality(actor, target, id, revision)
                .await
                .map(|session| CutSubmission::Quality(Box::new(session)))
                .map_err(map_store);
        }
        self.cut_confirm(actor, target, id, revision)
            .await
            .map(CutSubmission::Admitted)
    }

    pub async fn cut_confirm(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<Admission, DownloadError> {
        self.authorize_cut(actor, target).await?;
        let session = self
            .store
            .cut_claim(actor, target, id, revision)
            .await
            .map_err(map_store)?;
        let clips = session.clips.clone();
        if clips.is_empty() || clips.len() > 20 {
            return Err(DownloadError::InvalidInput);
        }
        let single = (clips.len() == 1).then_some(clips[0]);
        let request = if let Some(original) = session.original_request() {
            let params = if single.is_some() {
                original.params().clone()
            } else {
                original
                    .params()
                    .clone()
                    .with_clips(clips.clone())
                    .map_err(|_| DownloadError::InvalidInput)?
            };
            let mut request = DownloadRequest::new(
                actor,
                original.source_url().as_str(),
                MediaMode::Video,
                single,
                params,
            )
            .map_err(|_| DownloadError::InvalidInput)?;
            if let Some(target) = original.target().copied() {
                request = request
                    .with_target(target)
                    .map_err(|_| DownloadError::InvalidInput)?;
            }
            request
                .with_network(original.network().clone())
                .map_err(|_| DownloadError::InvalidInput)?
        } else {
            DownloadRequest::new(
                actor,
                session.source_url().ok_or(DownloadError::InvalidInput)?,
                MediaMode::Video,
                single,
                if single.is_some() {
                    DownloadParams::default()
                } else {
                    DownloadParams::default()
                        .with_clips(clips.clone())
                        .map_err(|_| DownloadError::InvalidInput)?
                },
            )
            .map_err(|_| DownloadError::InvalidInput)?
            .with_target(session.target)
            .map_err(|_| DownloadError::InvalidInput)?
        };
        // The identity belongs to the draft, not to the Telegram callback delivery.
        // A crash after admission leaves Confirming, whose retry finds the frozen request.
        let key = format!("cut:{}:{}:{}", target.bot_id(), actor, id.simple());
        let mut overrides = session
            .original_request()
            .and_then(|request| request.preference_overrides().cloned())
            .unwrap_or_default();
        if let Some(choice) = session.selected {
            overrides.max_height = Some(choice.max_height());
        }
        let admission = self
            .submit_preferred_request(&request, &key, overrides)
            .await?;
        self.store
            .cut_submitted(actor, target, id, revision, admission.operation_id)
            .await
            .map_err(map_store)?;
        Ok(admission)
    }
}
