use turso::transaction::{Transaction, TransactionBehavior};
use uuid::Uuid;
use yoink_db::{Row, query};
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode, NetworkSelection};

use crate::{Store, StoreError, codec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualityChoice {
    Best,
    Audio,
    Height(u32),
}
impl QualityChoice {
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "best" => Ok(Self::Best),
            "audio" => Ok(Self::Audio),
            "2160" => Ok(Self::Height(2160)),
            "1440" => Ok(Self::Height(1440)),
            "1080" => Ok(Self::Height(1080)),
            "720" => Ok(Self::Height(720)),
            "480" => Ok(Self::Height(480)),
            "360" => Ok(Self::Height(360)),
            "240" => Ok(Self::Height(240)),
            "144" => Ok(Self::Height(144)),
            _ => Err(StoreError::InvalidInput),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Best => "best",
            Self::Audio => "audio",
            Self::Height(2160) => "2160",
            Self::Height(1440) => "1440",
            Self::Height(1080) => "1080",
            Self::Height(720) => "720",
            Self::Height(480) => "480",
            Self::Height(360) => "360",
            Self::Height(240) => "240",
            Self::Height(144) => "144",
            Self::Height(_) => "",
        }
    }
    pub fn max_height(self) -> Option<u32> {
        match self {
            Self::Best | Self::Audio => None,
            Self::Height(h) => Some(h),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualityStage {
    Choosing,
    Confirming,
    Submitted,
    Cancelled,
    Cutting,
}
impl QualityStage {
    fn text(self) -> &'static str {
        match self {
            Self::Choosing => "choosing",
            Self::Confirming => "confirming",
            Self::Submitted => "submitted",
            Self::Cancelled => "cancelled",
            Self::Cutting => "cutting",
        }
    }
    fn parse(s: &str) -> Result<Self, StoreError> {
        match s {
            "choosing" => Ok(Self::Choosing),
            "confirming" => Ok(Self::Confirming),
            "submitted" => Ok(Self::Submitted),
            "cancelled" => Ok(Self::Cancelled),
            "cutting" => Ok(Self::Cutting),
            _ => Err(StoreError::InvalidStoredData),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct QualitySession {
    pub id: Uuid,
    pub revision: i64,
    pub stage: QualityStage,
    pub target: DeliveryTarget,
    pub origin_message_id: i32,
    pub selected: Option<QualityChoice>,
    pub operation_id: Option<Uuid>,
    pub cut_session_id: Option<Uuid>,
    pub(crate) request: DownloadRequest,
    pub(crate) last_message_id: i32,
}
impl std::fmt::Debug for QualitySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QualitySession")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("stage", &self.stage)
            .field("target", &self.target)
            .field("origin_message_id", &self.origin_message_id)
            .field("selected", &self.selected)
            .field("operation_id", &self.operation_id)
            .field("cut_session_id", &self.cut_session_id)
            .field("request", &"[REDACTED]")
            .finish()
    }
}
impl QualitySession {
    pub fn request(&self) -> &DownloadRequest {
        &self.request
    }
}

impl Store {
    pub async fn quality_cut(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<crate::CutSession, StoreError> {
        validate_revision(actor, target, revision)?;
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let row = query(include_str!("../sql/quality_get_for_target.sql"))
            .bind(actor as i64)
            .bind(target.bot_id())
            .bind(target.chat_id())
            .fetch_optional(&*tx)
            .await
            .map_err(|_| StoreError::Database)?
            .ok_or(StoreError::NotFoundOrStale)?;
        let mut quality = decode(row)?;
        if quality.id != id {
            return Err(StoreError::Conflict);
        }
        if quality.stage == QualityStage::Cutting {
            if revision != quality.revision && revision != quality.revision - 1 {
                return Err(StoreError::Conflict);
            }
            let linked = quality
                .cut_session_id
                .ok_or(StoreError::InvalidStoredData)?;
            let cut = crate::cut::current_in_transaction(&tx, actor, target)
                .await?
                .ok_or(StoreError::Conflict)?;
            if cut.id != linked || cut.linked_quality_id != Some(id) {
                return Err(StoreError::Conflict);
            }
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(cut);
        }
        if quality.stage != QualityStage::Choosing || quality.revision != revision {
            return Err(StoreError::Conflict);
        }
        if quality.request.params().playlist().is_some()
            || !quality.request.params().clips().is_empty()
        {
            return Err(StoreError::InvalidInput);
        }
        let active = crate::cut::current_in_transaction(&tx, actor, target).await?;
        if let Some(existing) = &active {
            if existing.last_message_id >= quality.origin_message_id {
                return Err(StoreError::Conflict);
            }
            if matches!(
                existing.stage,
                crate::CutStage::Start
                    | crate::CutStage::End
                    | crate::CutStage::Ready
                    | crate::CutStage::Quality
                    | crate::CutStage::Confirming
            ) {
                return Err(StoreError::Conflict);
            }
        }
        let request = quality.request.clone();
        let range = request.clip_range();
        let (start, end, stage) = match range {
            Some(range) => (
                Some(range.start_seconds()),
                Some(range.end_seconds()),
                crate::CutStage::Ready,
            ),
            None => (None, None, crate::CutStage::Start),
        };
        let cut_id = Uuid::new_v4();
        let cut = crate::CutSession {
            id: cut_id,
            revision: 1,
            stage,
            actor_id: actor,
            target: DeliveryTarget::new(
                target.bot_id(),
                target.chat_id(),
                None,
                Some(quality.origin_message_id),
            )
            .map_err(|_| StoreError::InvalidInput)?,
            origin_message_id: quality.origin_message_id,
            last_message_id: quality.origin_message_id,
            start,
            end,
            operation_id: None,
            selected: None,
            begin_start: start,
            url: Some(request.source_url().as_str().to_owned()),
            original: Some(Box::new(request)),
            linked_quality_id: Some(id),
        };
        quality.stage = QualityStage::Cutting;
        quality.revision = quality
            .revision
            .checked_add(1)
            .ok_or(StoreError::InvalidInput)?;
        quality.cut_session_id = Some(cut_id);
        put(&tx, actor, &quality).await?;
        crate::cut::put_handoff(&tx, &cut).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(cut)
    }

    pub async fn quality_current(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<Option<QualitySession>, StoreError> {
        validate(actor, target)?;
        let row = query(
            "SELECT * FROM runtime_quality_sessions WHERE actor_id=?1 AND bot_id=?2 AND chat_id=?3",
        )
        .bind(actor as i64)
        .bind(target.bot_id())
        .bind(target.chat_id())
        .fetch_optional(&self.database)
        .await
        .map_err(|_| StoreError::Database)?;
        row.map(decode).transpose()
    }

    pub async fn quality_begin(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message: i32,
        request: &DownloadRequest,
    ) -> Result<QualitySession, StoreError> {
        validate(actor, target)?;
        if message <= 0
            || request.actor_id() != actor
            || request.source_url().as_str().len() > 4096
            || request.media_mode() != MediaMode::Video
            || !is_quality_draft(request)
            || request.network() != &NetworkSelection::NONE
            || request.target().is_none_or(|t| {
                t.bot_id() != target.bot_id()
                    || t.chat_id() != target.chat_id()
                    || t.thread_id().is_some()
                    || t.reply_to_message_id() != Some(message)
            })
        {
            return Err(StoreError::InvalidInput);
        }
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let old = query(
            "SELECT * FROM runtime_quality_sessions WHERE actor_id=?1 AND bot_id=?2 AND chat_id=?3",
        )
        .bind(actor as i64)
        .bind(target.bot_id())
        .bind(target.chat_id())
        .fetch_optional(&*tx)
        .await
        .map_err(|_| StoreError::Database)?;
        if let Some(row) = old {
            let prior = decode(row)?;
            if message <= prior.last_message_id {
                if message == prior.origin_message_id
                    && codec::encode(&prior.request)? != codec::encode(request)?
                {
                    return Err(StoreError::Conflict);
                }
                tx.commit().await.map_err(|_| StoreError::Database)?;
                return Ok(prior);
            }
            if prior.stage == QualityStage::Confirming {
                return Err(StoreError::Conflict);
            }
        }
        let s = QualitySession {
            id: Uuid::new_v4(),
            revision: 1,
            stage: QualityStage::Choosing,
            target: DeliveryTarget::new(target.bot_id(), target.chat_id(), None, Some(message))
                .map_err(|_| StoreError::InvalidInput)?,
            origin_message_id: message,
            selected: None,
            operation_id: None,
            cut_session_id: None,
            request: request.clone(),
            last_message_id: message,
        };
        put(&tx, actor, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn quality_claim(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        choice: QualityChoice,
    ) -> Result<QualitySession, StoreError> {
        validate_revision(actor, target, revision)?;
        if QualityChoice::parse(choice.as_str()).is_err() {
            return Err(StoreError::InvalidInput);
        }
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = get(&tx, actor, target, id, revision).await?;
        if s.stage == QualityStage::Confirming
            && s.selected == Some(choice)
            && (revision == s.revision || revision == s.revision - 1)
        {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(s);
        }
        if s.stage != QualityStage::Choosing {
            return Err(StoreError::Conflict);
        }
        s.stage = QualityStage::Confirming;
        s.revision += 1;
        s.selected = Some(choice);
        put(&tx, actor, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }
    pub async fn quality_cancel(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<QualitySession, StoreError> {
        validate_revision(actor, target, revision)?;
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = get(&tx, actor, target, id, revision).await?;
        if s.stage == QualityStage::Cancelled && revision == s.revision - 1 {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(s);
        }
        if s.stage != QualityStage::Choosing {
            return Err(StoreError::Conflict);
        }
        s.stage = QualityStage::Cancelled;
        s.revision += 1;
        put(&tx, actor, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }
    pub async fn quality_submitted(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        operation: Uuid,
    ) -> Result<QualitySession, StoreError> {
        validate_revision(actor, target, revision)?;
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = get(&tx, actor, target, id, revision).await?;
        if s.stage == QualityStage::Submitted && s.operation_id == Some(operation) {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(s);
        }
        if s.stage != QualityStage::Confirming || revision != s.revision {
            return Err(StoreError::Conflict);
        }
        s.stage = QualityStage::Submitted;
        s.revision += 1;
        s.operation_id = Some(operation);
        put(&tx, actor, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }
}
// Structural selections belong to the original request. Format overrides remain
// separate until the owner chooses a height and the service resolves preferences.
fn is_quality_draft(request: &DownloadRequest) -> bool {
    let params = request.params();
    let defaults = DownloadParams::default()
        .with_engine(params.engine())
        .with_playlist(params.playlist())
        .with_clips(params.clips().to_vec());
    defaults.as_ref().is_ok_and(|expected| expected == params)
        && request.preference_overrides().is_none_or(|overrides| {
            overrides.max_height.is_none()
                && overrides.gallery_zip.is_none()
                && overrides.subtitle_language.is_none()
                && overrides.subtitle_automatic.is_none()
        })
}

fn validate(actor: u64, target: DeliveryTarget) -> Result<(), StoreError> {
    if actor == 0
        || actor > i64::MAX as u64
        || target.chat_id() <= 0
        || target.chat_id() as u64 != actor
        || target.thread_id().is_some()
    {
        Err(StoreError::InvalidInput)
    } else {
        Ok(())
    }
}
fn validate_revision(actor: u64, target: DeliveryTarget, revision: i64) -> Result<(), StoreError> {
    validate(actor, target)?;
    if revision <= 0 {
        Err(StoreError::InvalidInput)
    } else {
        Ok(())
    }
}
async fn get(
    tx: &turso::Connection,
    actor: u64,
    target: DeliveryTarget,
    id: Uuid,
    revision: i64,
) -> Result<QualitySession, StoreError> {
    validate_revision(actor, target, revision)?;
    let row = query(
        "SELECT * FROM runtime_quality_sessions WHERE actor_id=?1 AND bot_id=?2 AND chat_id=?3",
    )
    .bind(actor as i64)
    .bind(target.bot_id())
    .bind(target.chat_id())
    .fetch_optional(tx)
    .await
    .map_err(|_| StoreError::Database)?
    .ok_or(StoreError::NotFoundOrStale)?;
    let s = decode(row)?;
    if s.id != id
        || !(s.revision == revision
            || (matches!(s.stage, QualityStage::Confirming | QualityStage::Cancelled)
                && s.revision - 1 == revision))
    {
        Err(StoreError::NotFoundOrStale)
    } else {
        Ok(s)
    }
}
async fn put(tx: &turso::Connection, actor: u64, s: &QualitySession) -> Result<(), StoreError> {
    let request = codec::encode(&s.request)?;
    query(include_str!("../sql/quality_put.sql"))
        .bind(actor as i64)
        .bind(s.target.bot_id())
        .bind(s.target.chat_id())
        .bind(s.id.to_string())
        .bind(s.revision)
        .bind(s.stage.text())
        .bind(s.origin_message_id)
        .bind(s.last_message_id)
        .bind(request.to_string())
        .bind(s.selected.map(QualityChoice::as_str))
        .bind(s.operation_id.map(|x| x.to_string()))
        .bind(s.cut_session_id.map(|x| x.to_string()))
        .execute(tx)
        .await
        .map_err(|_| StoreError::Database)?;
    Ok(())
}
fn decode(row: Row) -> Result<QualitySession, StoreError> {
    let target = DeliveryTarget::new(
        row.try_get("bot_id")
            .map_err(|_| StoreError::InvalidStoredData)?,
        row.try_get("chat_id")
            .map_err(|_| StoreError::InvalidStoredData)?,
        None,
        Some(
            row.try_get("origin_message_id")
                .map_err(|_| StoreError::InvalidStoredData)?,
        ),
    )
    .map_err(|_| StoreError::InvalidStoredData)?;
    let raw: String = row
        .try_get("request_json")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let request =
        codec::decode(serde_json::from_str(&raw).map_err(|_| StoreError::InvalidStoredData)?)?;
    let actor: i64 = row
        .try_get("actor_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    if actor <= 0
        || request.actor_id() != actor as u64
        || request.media_mode() != MediaMode::Video
        || !is_quality_draft(&request)
        || request.network() != &NetworkSelection::NONE
        || request.source_url().as_str().len() > 4096
        || request.target().is_none_or(|t| {
            t.bot_id() != target.bot_id()
                || t.chat_id() != target.chat_id()
                || t.thread_id().is_some()
                || t.reply_to_message_id() != target.reply_to_message_id()
        })
    {
        return Err(StoreError::InvalidStoredData);
    }
    let choice: Option<String> = row
        .try_get("selected")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let op: Option<String> = row
        .try_get("operation_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let id = Uuid::parse_str(
        &row.try_get::<String, _>("id")
            .map_err(|_| StoreError::InvalidStoredData)?,
    )
    .map_err(|_| StoreError::InvalidStoredData)?;
    if row
        .try_get::<String, _>("id")
        .map_err(|_| StoreError::InvalidStoredData)?
        != id.to_string()
    {
        return Err(StoreError::InvalidStoredData);
    }
    let revision: i64 = row
        .try_get("revision")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let stage = QualityStage::parse(
        &row.try_get::<String, _>("stage")
            .map_err(|_| StoreError::InvalidStoredData)?,
    )?;
    let origin_message_id: i32 = row
        .try_get("origin_message_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let last_message_id: i32 = row
        .try_get("last_message_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let selected = choice.map(|s| QualityChoice::parse(&s)).transpose()?;
    let operation_id = op
        .as_ref()
        .map(|s| Uuid::parse_str(s).map_err(|_| StoreError::InvalidStoredData))
        .transpose()?;
    let linked_raw: Option<String> = row
        .try_get("linked_cut_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let cut_session_id = linked_raw
        .as_ref()
        .map(|v| Uuid::parse_str(v).map_err(|_| StoreError::InvalidStoredData))
        .transpose()?;
    if matches!(stage, QualityStage::Cutting) != cut_session_id.is_some() {
        return Err(StoreError::InvalidStoredData);
    }
    if let (Some(raw), Some(parsed)) = (op.as_deref(), operation_id)
        && raw != parsed.to_string()
    {
        return Err(StoreError::InvalidStoredData);
    }
    let expected_revision = match stage {
        QualityStage::Choosing => 1,
        QualityStage::Confirming | QualityStage::Cancelled => 2,
        QualityStage::Submitted => 3,
        QualityStage::Cutting => 2,
    };
    if revision != expected_revision
        || origin_message_id <= 0
        || last_message_id < origin_message_id
        || (matches!(stage, QualityStage::Confirming | QualityStage::Submitted)
            != selected.is_some())
        || (matches!(stage, QualityStage::Submitted) != operation_id.is_some())
    {
        return Err(StoreError::InvalidStoredData);
    }
    Ok(QualitySession {
        id,
        revision,
        stage,
        target,
        origin_message_id,
        last_message_id,
        selected,
        operation_id,
        cut_session_id,
        request,
    })
}

#[cfg(test)]
mod tests {
    use super::QualityChoice;

    #[test]
    fn parser_accepts_only_the_public_labels() {
        for label in [
            "best", "audio", "2160", "1440", "1080", "720", "480", "360", "240", "144",
        ] {
            assert_eq!(QualityChoice::parse(label).unwrap().as_str(), label);
        }
        for label in ["ASK", "Best", "2160p", "4320", "0", "", "360 "] {
            assert!(QualityChoice::parse(label).is_err());
        }
    }
}
