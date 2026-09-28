use std::time::Duration;

use uuid::Uuid;
use yoink_storage::{ProgressMessage, ProgressNotice};

use crate::{DownloadError, DownloadService, map_store};

impl DownloadService {
    /// Track only an acknowledged admission reply to this operation's authorized destination.
    pub async fn register_progress_message(
        &self,
        actor_id: u64,
        id: Uuid,
        message_id: i32,
        language: &str,
    ) -> Result<bool, DownloadError> {
        let record = self
            .status(actor_id, id)
            .await?
            .ok_or(DownloadError::Denied)?;
        let target = record.target.ok_or(DownloadError::Denied)?;
        self.store
            .register_progress_message(&ProgressMessage {
                operation_id: id,
                actor_id,
                bot_id: target.bot_id(),
                chat_id: target.chat_id(),
                message_id,
                language: language.to_owned(),
            })
            .await
            .map_err(map_store)
    }

    pub async fn due_progress_notice(&self) -> Result<Option<ProgressNotice>, DownloadError> {
        self.store
            .due_progress_notice(self.delivery.bot_id())
            .await
            .map_err(map_store)
    }

    /// Call status again before each external edit, not just when registering a notice.
    pub async fn advance_progress_notice(
        &self,
        notice: &ProgressNotice,
        sent_text: Option<&str>,
        finished: bool,
        delay: Duration,
    ) -> Result<bool, DownloadError> {
        if notice.message.bot_id != self.delivery.bot_id() || delay > Duration::from_secs(86400) {
            return Err(DownloadError::InvalidInput);
        }
        let next = chrono::Utc::now()
            + chrono::Duration::from_std(delay).map_err(|_| DownloadError::InvalidInput)?;
        self.store
            .advance_progress_notice(&notice.message, sent_text, finished, next)
            .await
            .map_err(map_store)
    }
}
