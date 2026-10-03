use crate::QualityChoice;
use crate::codec;
use turso::transaction::{Transaction, TransactionBehavior};
use uuid::Uuid;
use yoink_db::{Row, query};
use yoink_domain::{
    ClipRange, DeliveryTarget, DownloadParams, DownloadRequest, MediaMode, NetworkSelection,
};

use crate::{Store, StoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CutStage {
    Start,
    End,
    Ready,
    Quality,
    Confirming,
    Submitted,
    Cancelled,
}

impl CutStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
            Self::Ready => "ready",
            Self::Quality => "quality",
            Self::Confirming => "confirming",
            Self::Submitted => "submitted",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "start" => Ok(Self::Start),
            "end" => Ok(Self::End),
            "ready" => Ok(Self::Ready),
            "quality" => Ok(Self::Quality),
            "confirming" => Ok(Self::Confirming),
            "submitted" => Ok(Self::Submitted),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(StoreError::InvalidStoredData),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct CutSession {
    pub id: Uuid,
    pub revision: i64,
    pub stage: CutStage,
    pub actor_id: u64,
    pub target: DeliveryTarget,
    pub origin_message_id: i32,
    pub last_message_id: i32,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub clips: Vec<ClipRange>,
    pub editing_index: Option<usize>,
    pub operation_id: Option<Uuid>,
    pub selected: Option<QualityChoice>,
    pub(crate) begin_start: Option<f64>,
    pub(crate) url: Option<String>,
    pub(crate) original: Option<Box<DownloadRequest>>,
    pub(crate) linked_quality_id: Option<Uuid>,
}

impl CutSession {
    pub fn source_url(&self) -> Option<&str> {
        self.url.as_deref()
    }
    pub fn original_request(&self) -> Option<&DownloadRequest> {
        self.original.as_deref()
    }
    pub fn linked_quality_id(&self) -> Option<Uuid> {
        self.linked_quality_id
    }
}

impl std::fmt::Debug for CutSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CutSession")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("stage", &self.stage)
            .field("actor_id", &self.actor_id)
            .field("target", &self.target)
            .field("origin_message_id", &self.origin_message_id)
            .field("last_message_id", &self.last_message_id)
            .field("source_url", &self.url.as_ref().map(|_| "[REDACTED]"))
            .field("start", &self.start)
            .field("end", &self.end)
            .field("clips", &self.clips)
            .field("editing_index", &self.editing_index)
            .field("operation_id", &self.operation_id)
            .field("selected", &self.selected)
            .finish()
    }
}

impl Store {
    pub async fn cut_current(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<Option<CutSession>, StoreError> {
        validate_scope(actor, target)?;
        let row = query(include_str!("../sql/cut_get.sql"))
            .bind(actor as i64)
            .bind(target.bot_id())
            .bind(target.chat_id())
            .fetch_optional(&self.database)
            .await
            .map_err(|_| StoreError::Database)?;
        row.map(decode).transpose()
    }

    pub async fn cut_begin(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        url: &str,
        start: Option<f64>,
    ) -> Result<CutSession, StoreError> {
        validate_scope(actor, target)?;
        if message_id <= 0 {
            return Err(StoreError::InvalidInput);
        }
        let request = DownloadRequest::new(
            actor,
            url,
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .map_err(|_| StoreError::InvalidInput)?;
        if request.source_url().as_str().len() > 4096 {
            return Err(StoreError::InvalidInput);
        }
        if start.is_some_and(|v| !valid_time(v)) {
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
        if let Some(row) = get(&tx, actor, target).await? {
            let old = decode(row)?;
            if message_id <= old.last_message_id {
                if message_id == old.origin_message_id
                    && (old.source_url() != Some(url) || old.begin_start != start)
                {
                    return Err(StoreError::Conflict);
                }
                tx.commit().await.map_err(|_| StoreError::Database)?;
                return Ok(old);
            }
            if old.stage == CutStage::Confirming {
                return Err(StoreError::Conflict);
            }
        }
        let request_target =
            DeliveryTarget::new(target.bot_id(), target.chat_id(), None, Some(message_id))
                .map_err(|_| StoreError::InvalidInput)?;
        let session = CutSession {
            id: Uuid::new_v4(),
            revision: 1,
            stage: if start.is_some() {
                CutStage::End
            } else {
                CutStage::Start
            },
            actor_id: actor,
            target: request_target,
            origin_message_id: message_id,
            last_message_id: message_id,
            start,
            end: None,
            clips: Vec::new(),
            editing_index: None,
            operation_id: None,
            selected: None,
            begin_start: start,
            url: Some(url.to_owned()),
            original: None,
            linked_quality_id: None,
        };
        put(&tx, &session).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(session)
    }

    pub async fn cut_input(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        value: f64,
        numeric: bool,
    ) -> Result<Option<CutSession>, StoreError> {
        validate_scope(actor, target)?;
        if message_id <= 0 || !valid_time(value) {
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
        let Some(row) = get(&tx, actor, target).await? else {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(None);
        };
        let mut s = decode(row)?;
        if message_id <= s.last_message_id {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(Some(s));
        }
        let prior_stage = s.stage;
        match s.stage {
            CutStage::Start => {
                s.start = Some(value);
                s.stage = CutStage::End;
            }
            CutStage::End => {
                let start = s.start.ok_or(StoreError::InvalidStoredData)?;
                let end = if numeric && value <= start {
                    start + value
                } else {
                    value
                };
                let range = ClipRange::new(start, end).map_err(|_| StoreError::InvalidInput)?;
                if let Some(index) = s.editing_index {
                    if index >= s.clips.len() {
                        return Err(StoreError::InvalidStoredData);
                    }
                    s.clips[index] = range;
                } else {
                    if s.clips.len() >= 20 {
                        return Err(StoreError::InvalidInput);
                    }
                    s.clips.push(range);
                }
                s.editing_index = None;
                s.start = s.clips.first().map(ClipRange::start_seconds);
                s.end = s.clips.first().map(ClipRange::end_seconds);
                s.stage = CutStage::Ready;
            }
            _ => return Err(StoreError::InvalidInput),
        }
        if s.stage != prior_stage {
            s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        }
        s.last_message_id = message_id;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(Some(s))
    }

    pub async fn cut_edit(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if !matches!(s.stage, CutStage::Ready | CutStage::Quality) {
            return Err(StoreError::Conflict);
        }
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        s.stage = CutStage::Start;
        s.start = None;
        s.end = None;
        s.editing_index = Some(0);
        s.operation_id = None;
        s.selected = None;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_add(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, StoreError> {
        self.cut_begin_segment(actor, target, id, revision, None, true)
            .await
    }

    pub async fn cut_edit_segment(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        index: usize,
    ) -> Result<CutSession, StoreError> {
        self.cut_begin_segment(actor, target, id, revision, Some(index), false)
            .await
    }

    async fn cut_begin_segment(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        index: Option<usize>,
        append: bool,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if !matches!(s.stage, CutStage::Ready | CutStage::Quality) {
            return Err(StoreError::Conflict);
        }
        if append {
            if s.clips.len() >= 20 {
                return Err(StoreError::InvalidInput);
            }
        } else if index.is_none_or(|i| i >= s.clips.len()) {
            return Err(StoreError::InvalidInput);
        }
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        s.stage = CutStage::Start;
        s.start = None;
        s.end = None;
        s.editing_index = index;
        s.selected = None;
        s.operation_id = None;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_delete_segment(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        index: usize,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if !matches!(s.stage, CutStage::Ready | CutStage::Quality) {
            return Err(StoreError::Conflict);
        }
        if index >= s.clips.len() {
            return Err(StoreError::InvalidInput);
        }
        s.clips.remove(index);
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        s.selected = None;
        s.operation_id = None;
        s.editing_index = None;
        s.start = s.clips.first().map(ClipRange::start_seconds);
        s.end = s.clips.first().map(ClipRange::end_seconds);
        if s.clips.is_empty() {
            s.stage = CutStage::Start;
            s.start = None;
            s.end = None;
        } else {
            s.stage = CutStage::Ready;
        }
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_back(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if !matches!(s.stage, CutStage::Start | CutStage::End) || s.clips.is_empty() {
            return Err(StoreError::Conflict);
        }
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        s.stage = CutStage::Ready;
        s.start = s.clips.first().map(ClipRange::start_seconds);
        s.end = s.clips.first().map(ClipRange::end_seconds);
        s.editing_index = None;
        s.selected = None;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_cancel(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        message_id: Option<i32>,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if s.stage == CutStage::Cancelled {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(s);
        }
        if !matches!(
            s.stage,
            CutStage::Start | CutStage::End | CutStage::Ready | CutStage::Quality
        ) {
            return Err(StoreError::Conflict);
        }
        if let Some(mid) = message_id {
            if mid <= 0 {
                return Err(StoreError::InvalidInput);
            }
            if mid <= s.last_message_id {
                return Err(StoreError::Conflict);
            }
            s.last_message_id = mid;
        }
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        s.stage = CutStage::Cancelled;
        s.start = None;
        s.end = None;
        s.selected = None;
        s.clips.clear();
        s.editing_index = None;
        s.original = None;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_claim(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        match s.stage {
            CutStage::Ready => s.stage = CutStage::Confirming,
            CutStage::Quality if s.selected.is_some() => s.stage = CutStage::Confirming,
            CutStage::Confirming | CutStage::Submitted => {
                tx.commit().await.map_err(|_| StoreError::Database)?;
                return Ok(s);
            }
            _ => return Err(StoreError::Conflict),
        }
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_quality(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if s.stage != CutStage::Ready {
            return Err(StoreError::Conflict);
        }
        s.stage = CutStage::Quality;
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_choose(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        choice: QualityChoice,
    ) -> Result<CutSession, StoreError> {
        if QualityChoice::parse(choice.as_str()).is_err() || choice == QualityChoice::Audio {
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
        let mut s = require_choice_replay(&tx, actor, target, id, revision).await?;
        if matches!(s.stage, CutStage::Confirming | CutStage::Submitted)
            && s.selected == Some(choice)
            && (revision == s.revision || revision == s.revision - 1)
        {
            tx.commit().await.map_err(|_| StoreError::Database)?;
            return Ok(s);
        }
        if s.stage != CutStage::Quality {
            return Err(StoreError::Conflict);
        }
        s.stage = CutStage::Confirming;
        s.selected = Some(choice);
        s.revision = s.revision.checked_add(1).ok_or(StoreError::InvalidInput)?;
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }

    pub async fn cut_submitted(
        &self,
        actor: u64,
        target: DeliveryTarget,
        id: Uuid,
        revision: i64,
        operation_id: Uuid,
    ) -> Result<CutSession, StoreError> {
        let mut conn = self
            .database
            .connection()
            .await
            .map_err(|_| StoreError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| StoreError::Database)?;
        let mut s = require(&tx, actor, target, id, revision).await?;
        if s.stage == CutStage::Submitted {
            if s.operation_id == Some(operation_id) {
                tx.commit().await.map_err(|_| StoreError::Database)?;
                return Ok(s);
            }
            return Err(StoreError::Conflict);
        }
        if s.stage != CutStage::Confirming {
            return Err(StoreError::Conflict);
        }
        s.stage = CutStage::Submitted;
        s.operation_id = Some(operation_id);
        put(&tx, &s).await?;
        tx.commit().await.map_err(|_| StoreError::Database)?;
        Ok(s)
    }
}

async fn require_choice_replay(
    tx: &Transaction<'_>,
    actor: u64,
    target: DeliveryTarget,
    id: Uuid,
    revision: i64,
) -> Result<CutSession, StoreError> {
    // Unlike ordinary revision-bound edits, a delivered choice callback may be replayed
    // after its transition committed. Decode the current row, then validate exact revision below.
    validate_scope(actor, target)?;
    let s = get(tx, actor, target)
        .await?
        .map(decode)
        .transpose()?
        .ok_or(StoreError::Conflict)?;
    let replay =
        matches!(s.stage, CutStage::Confirming | CutStage::Submitted) && revision == s.revision - 1;
    if s.id != id || (s.revision != revision && !replay) {
        return Err(StoreError::Conflict);
    }
    Ok(s)
}

fn validate_scope(actor: u64, target: DeliveryTarget) -> Result<(), StoreError> {
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
fn valid_time(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}
async fn get(
    tx: &turso::transaction::Transaction<'_>,
    actor: u64,
    target: DeliveryTarget,
) -> Result<Option<Row>, StoreError> {
    query(include_str!("../sql/cut_get.sql"))
        .bind(actor as i64)
        .bind(target.bot_id())
        .bind(target.chat_id())
        .fetch_optional(&**tx)
        .await
        .map_err(|_| StoreError::Database)
}
pub(crate) async fn current_in_transaction(
    tx: &Transaction<'_>,
    actor: u64,
    target: DeliveryTarget,
) -> Result<Option<CutSession>, StoreError> {
    get(tx, actor, target).await?.map(decode).transpose()
}
async fn require(
    tx: &turso::transaction::Transaction<'_>,
    actor: u64,
    target: DeliveryTarget,
    id: Uuid,
    revision: i64,
) -> Result<CutSession, StoreError> {
    validate_scope(actor, target)?;
    let s = get(tx, actor, target)
        .await?
        .map(decode)
        .transpose()?
        .ok_or(StoreError::Conflict)?;
    if s.actor_id != actor
        || s.target.bot_id() != target.bot_id()
        || s.target.chat_id() != target.chat_id()
        || s.id != id
        || s.revision != revision
    {
        return Err(StoreError::Conflict);
    }
    Ok(s)
}
async fn put(tx: &turso::transaction::Transaction<'_>, s: &CutSession) -> Result<(), StoreError> {
    query(include_str!("../sql/cut_put.sql"))
        .bind(s.actor_id as i64)
        .bind(s.target.bot_id())
        .bind(s.target.chat_id())
        .bind(s.id)
        .bind(s.revision)
        .bind(s.stage.as_str())
        .bind(s.origin_message_id)
        .bind(s.last_message_id)
        .bind(s.url.as_deref())
        .bind(s.begin_start)
        .bind(s.start)
        .bind(s.end)
        .bind(s.operation_id)
        .bind(s.selected.map(QualityChoice::as_str))
        .bind(
            s.original
                .as_ref()
                .map(|r| codec::encode(r).map(|v| v.to_string()))
                .transpose()?,
        )
        .bind(s.linked_quality_id)
        .bind(
            serde_json::to_string(
                &s.clips
                    .iter()
                    .map(|clip| [clip.start_seconds(), clip.end_seconds()])
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| StoreError::InvalidInput)?,
        )
        .bind(s.editing_index.map(|index| index as i64))
        .execute(&**tx)
        .await
        .map_err(|_| StoreError::Database)?;
    Ok(())
}
fn decode(row: Row) -> Result<CutSession, StoreError> {
    let actor: i64 = row
        .try_get("actor_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let bot_id: i64 = row
        .try_get("bot_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let chat_id: i64 = row
        .try_get("chat_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let origin_message_id: i32 = row
        .try_get("origin_message_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let target = DeliveryTarget::new(bot_id, chat_id, None, Some(origin_message_id))
        .map_err(|_| StoreError::InvalidStoredData)?;
    let selected = row
        .try_get::<Option<String>, _>("selected")
        .map_err(|_| StoreError::InvalidStoredData)?
        .map(|value| QualityChoice::parse(&value))
        .transpose()?;
    if selected == Some(QualityChoice::Audio) {
        return Err(StoreError::InvalidStoredData);
    }
    let stage = CutStage::parse(
        &row.try_get::<String, _>("stage")
            .map_err(|_| StoreError::InvalidStoredData)?,
    )?;
    let url: Option<String> = row
        .try_get("source_url")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let original = row
        .try_get::<Option<String>, _>("original_request_json")
        .map_err(|_| StoreError::InvalidStoredData)?
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|_| StoreError::InvalidStoredData)
                .and_then(codec::decode)
                .map(Box::new)
        })
        .transpose()?;
    let linked_quality_raw: Option<String> = row
        .try_get("linked_quality_id")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let linked_quality_id = linked_quality_raw
        .as_deref()
        .map(|value| Uuid::parse_str(value).map_err(|_| StoreError::InvalidStoredData))
        .transpose()?;
    if let (Some(raw), Some(parsed)) = (linked_quality_raw.as_deref(), linked_quality_id)
        && raw != parsed.to_string()
    {
        return Err(StoreError::InvalidStoredData);
    }
    if let Some(request) = &original {
        let request_target = request.target().ok_or(StoreError::InvalidStoredData)?;
        if request.actor_id() != actor as u64
            || request.media_mode() != MediaMode::Video
            || request.network() != &NetworkSelection::NONE
            || request.params().playlist().is_some()
            || !request.params().clips().is_empty()
            || request.source_url().as_str()
                != url.as_deref().ok_or(StoreError::InvalidStoredData)?
            || request_target.bot_id() != bot_id
            || request_target.chat_id() != chat_id
            || request_target.thread_id().is_some()
            || request_target.reply_to_message_id() != Some(origin_message_id)
            || linked_quality_id.is_none()
        {
            return Err(StoreError::InvalidStoredData);
        }
    }
    if linked_quality_id.is_some() && original.is_none() && stage != CutStage::Cancelled {
        return Err(StoreError::InvalidStoredData);
    }
    let clips_raw: String = row
        .try_get("clips_json")
        .map_err(|_| StoreError::InvalidStoredData)?;
    let stored_clips: Vec<[f64; 2]> =
        serde_json::from_str(&clips_raw).map_err(|_| StoreError::InvalidStoredData)?;
    if stored_clips.len() > 20 {
        return Err(StoreError::InvalidStoredData);
    }
    let clips = stored_clips
        .into_iter()
        .map(|[start, end]| ClipRange::new(start, end).map_err(|_| StoreError::InvalidStoredData))
        .collect::<Result<Vec<_>, _>>()?;
    let editing_index = row
        .try_get::<Option<i64>, _>("editing_index")
        .map_err(|_| StoreError::InvalidStoredData)?
        .map(|index| usize::try_from(index).map_err(|_| StoreError::InvalidStoredData))
        .transpose()?;
    if editing_index.is_some_and(|index| index >= clips.len())
        || (matches!(
            stage,
            CutStage::Ready | CutStage::Quality | CutStage::Confirming | CutStage::Submitted
        ) && clips.is_empty())
        || (stage == CutStage::Cancelled && !clips.is_empty())
        || (editing_index.is_some() && !matches!(stage, CutStage::Start | CutStage::End))
    {
        return Err(StoreError::InvalidStoredData);
    }
    Ok(CutSession {
        id: row
            .try_get("id")
            .map_err(|_| StoreError::InvalidStoredData)?,
        revision: row
            .try_get("revision")
            .map_err(|_| StoreError::InvalidStoredData)?,
        stage,
        actor_id: u64::try_from(actor).map_err(|_| StoreError::InvalidStoredData)?,
        target,
        origin_message_id,
        last_message_id: row
            .try_get("last_message_id")
            .map_err(|_| StoreError::InvalidStoredData)?,
        begin_start: row
            .try_get("begin_start")
            .map_err(|_| StoreError::InvalidStoredData)?,
        start: row
            .try_get("start")
            .map_err(|_| StoreError::InvalidStoredData)?,
        end: row
            .try_get("end")
            .map_err(|_| StoreError::InvalidStoredData)?,
        clips,
        editing_index,
        operation_id: row
            .try_get("operation_id")
            .map_err(|_| StoreError::InvalidStoredData)?,
        selected,
        url,
        original,
        linked_quality_id,
    })
}

pub(crate) async fn put_handoff(
    tx: &Transaction<'_>,
    session: &CutSession,
) -> Result<(), StoreError> {
    put(tx, session).await
}
