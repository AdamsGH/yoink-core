//! Persistent clip conversations. Transport adapters never own the admission recipe.
use uuid::Uuid;
use yoink_domain::{
    ClipRange, DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest,
    MediaMode,
};
use yoink_storage::Admission;

pub use yoink_storage::QualityChoice;
pub use yoink_storage::{CutSession, CutStage};

#[derive(Clone, Debug, PartialEq)]
pub enum CutSubmission {
    Admitted(Admission),
    Quality(CutSession),
}

use crate::{DownloadError, DownloadService, map_store};

impl DownloadService {
    async fn authorize_cut(&self, actor: u64, target: DeliveryTarget) -> Result<(), DownloadError> {
        if target.chat_id() <= 0 || target.chat_id() as u64 != actor || target.thread_id().is_some()
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
                .map(CutSubmission::Quality)
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
        let clip = ClipRange::new(
            session.start.ok_or(DownloadError::InvalidInput)?,
            session.end.ok_or(DownloadError::InvalidInput)?,
        )
        .map_err(|_| DownloadError::InvalidInput)?;
        let request = DownloadRequest::new(
            actor,
            session.source_url().ok_or(DownloadError::InvalidInput)?,
            MediaMode::Video,
            Some(clip),
            DownloadParams::default(),
        )
        .map_err(|_| DownloadError::InvalidInput)?
        .with_target(session.target)
        .map_err(|_| DownloadError::InvalidInput)?;
        // The identity belongs to the draft, not to the Telegram callback delivery.
        // A crash after admission leaves Confirming, whose retry finds the frozen request.
        let key = format!("cut:{}:{}:{}", target.bot_id(), actor, id.simple());
        let mut overrides = DownloadPreferenceOverrides::default();
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
