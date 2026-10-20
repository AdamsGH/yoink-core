use chrono::{DateTime, Utc};
use uuid::Uuid;
use yoink_db::query;

use crate::{Store, StoreError};

pub(crate) const INPUT_ONLY_NOTICE: &str = "[pending-input-cleanup]";

/// A confirmed message reference, never a request to send a new Telegram message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressMessage {
    pub operation_id: Uuid,
    pub actor_id: u64,
    pub bot_id: i64,
    pub chat_id: i64,
    pub message_id: i32,
    pub language: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressNotice {
    pub message: ProgressMessage,
    /// Empty text suppresses edits to a removed notice. Input-only references carry a reserved marker.
    pub last_text: Option<String>,
}

impl ProgressNotice {
    pub fn input_only(&self) -> bool {
        self.last_text.as_deref() == Some(INPUT_ONLY_NOTICE)
    }
}

impl ProgressMessage {
    fn validate(&self) -> Result<(), StoreError> {
        if self.actor_id == 0
            || self.actor_id > i64::MAX as u64
            || self.bot_id <= 0
            || self.chat_id == 0
            || self.message_id <= 0
            || !matches!(self.language.as_str(), "en" | "ru")
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }
}

impl Store {
    pub async fn operation_by_reply(
        &self,
        actor_id: u64,
        bot_id: i64,
        chat_id: i64,
        message_id: i32,
    ) -> Result<Option<Uuid>, StoreError> {
        if actor_id == 0
            || actor_id > i64::MAX as u64
            || bot_id <= 0
            || chat_id == 0
            || message_id <= 0
        {
            return Err(StoreError::InvalidInput);
        }
        query(include_str!("../sql/operation_by_reply.sql"))
            .bind(actor_id as i64)
            .bind(bot_id)
            .bind(chat_id)
            .bind(message_id)
            .fetch_optional(self.database())
            .await
            .map_err(|_| StoreError::Database)?
            .map(|row| row.try_get("id").map_err(|_| StoreError::InvalidStoredData))
            .transpose()
    }

    /// First confirmed message wins, including concurrent same-operation replays.
    pub async fn register_progress_message(
        &self,
        message: &ProgressMessage,
    ) -> Result<bool, StoreError> {
        message.validate()?;
        query(include_str!("../sql/register_progress_notice.sql"))
            .bind(message.operation_id)
            .bind(message.actor_id as i64)
            .bind(message.bot_id)
            .bind(message.chat_id)
            .bind(message.message_id)
            .bind(message.language.as_str())
            .bind(INPUT_ONLY_NOTICE)
            .fetch_optional(self.database())
            .await
            .map(|row| row.is_some())
            .map_err(|_| StoreError::Database)
    }

    pub async fn due_progress_notice(
        &self,
        bot_id: i64,
    ) -> Result<Option<ProgressNotice>, StoreError> {
        if bot_id <= 0 {
            return Err(StoreError::InvalidInput);
        }
        let row = query(include_str!("../sql/due_progress_notice.sql"))
            .bind(bot_id)
            .bind(Utc::now())
            .fetch_optional(self.database())
            .await
            .map_err(|_| StoreError::Database)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let message = ProgressMessage {
            operation_id: row
                .try_get("operation_id")
                .map_err(|_| StoreError::InvalidStoredData)?,
            actor_id: u64::try_from(
                row.try_get::<i64, _>("actor_id")
                    .map_err(|_| StoreError::InvalidStoredData)?,
            )
            .map_err(|_| StoreError::InvalidStoredData)?,
            bot_id: row
                .try_get("bot_id")
                .map_err(|_| StoreError::InvalidStoredData)?,
            chat_id: row
                .try_get("chat_id")
                .map_err(|_| StoreError::InvalidStoredData)?,
            message_id: row
                .try_get("message_id")
                .map_err(|_| StoreError::InvalidStoredData)?,
            language: row
                .try_get("language")
                .map_err(|_| StoreError::InvalidStoredData)?,
        };
        message
            .validate()
            .map_err(|_| StoreError::InvalidStoredData)?;
        Ok(Some(ProgressNotice {
            message,
            last_text: row
                .try_get("last_text")
                .map_err(|_| StoreError::InvalidStoredData)?,
        }))
    }

    /// Failed or unchanged edits retain the previous acknowledged text. Finishing is irreversible.
    pub async fn advance_progress_notice(
        &self,
        message: &ProgressMessage,
        sent_text: Option<&str>,
        finished: bool,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        message.validate()?;
        if sent_text.is_some_and(|text| text.len() > 4096) {
            return Err(StoreError::InvalidInput);
        }
        query(include_str!("../sql/advance_progress_notice.sql"))
            .bind(message.operation_id)
            .bind(message.actor_id as i64)
            .bind(message.bot_id)
            .bind(message.message_id)
            .bind(sent_text)
            .bind(finished)
            .bind(next_attempt_at)
            .fetch_optional(self.database())
            .await
            .map(|row| row.is_some())
            .map_err(|_| StoreError::Database)
    }
}
