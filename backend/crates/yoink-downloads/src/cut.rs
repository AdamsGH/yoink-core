//! Persistent clip conversations. Transport adapters never own the admission recipe.
use uuid::Uuid;
use yoink_domain::{
    ClipRange, DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest,
    MediaMode,
};
use yoink_storage::Admission;

pub use yoink_storage::{CutSession, CutStage};

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
        let admission = self
            .submit_preferred_request(&request, &key, DownloadPreferenceOverrides::default())
            .await?;
        self.store
            .cut_submitted(actor, target, id, revision, admission.operation_id)
            .await
            .map_err(map_store)?;
        Ok(admission)
    }
}
