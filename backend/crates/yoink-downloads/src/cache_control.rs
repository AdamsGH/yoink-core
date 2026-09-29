//! Owner-private cache controls, independent of saved format preferences.
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};

use crate::{DownloadError, DownloadService};

impl DownloadService {
    /// Clear every cached format for one exact canonical URL in this actor/bot partition.
    /// The source URL is intentionally absent from errors and debug output.
    pub async fn clear_media_cache(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        source_url: &str,
    ) -> Result<Option<u64>, DownloadError> {
        if actor == 0 || actor > i64::MAX as u64 {
            return Err(DownloadError::Denied);
        }
        if message_id <= 0
            || source_url.len() > 4096
            || source_url
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(DownloadError::InvalidInput);
        }
        if !self.identities.is_configured_owner(actor as i64) {
            return Err(DownloadError::Denied);
        }
        if target.chat_id() <= 0 || target.chat_id() as u64 != actor || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        self.authorize_chat(actor, target).await?;
        let request = DownloadRequest::new(
            actor,
            source_url,
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .map_err(|_| DownloadError::InvalidInput)?;
        self.store
            .clear_media_cache(
                actor,
                target.bot_id(),
                message_id,
                request.source_url().as_str(),
                true,
            )
            .await
            .map_err(crate::map_store)
    }
}
