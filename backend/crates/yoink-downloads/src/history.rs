//! Caller-owned imported and native download history.
use crate::{DownloadError, UserSettingsService};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use yoink_db::query;

impl crate::DownloadService {
    pub async fn retry_download(
        &self,
        actor: u64,
        source: &str,
    ) -> Result<yoink_storage::Admission, DownloadError> {
        let target = yoink_domain::DeliveryTarget::new(
            self.delivery.bot_id(),
            i64::try_from(actor).map_err(|_| DownloadError::InvalidInput)?,
            None,
            None,
        )
        .map_err(|_| DownloadError::InvalidInput)?;
        let request = yoink_domain::DownloadRequest::new(
            actor,
            source,
            yoink_domain::MediaMode::Video,
            None,
            yoink_domain::DownloadParams::default(),
        )
        .map_err(|_| DownloadError::InvalidInput)?
        .with_target(target)
        .map_err(|_| DownloadError::InvalidInput)?;
        self.submit_preferred_request(
            &request,
            &format!("history-retry:{}", uuid::Uuid::new_v4()),
            yoink_domain::DownloadPreferenceOverrides::default(),
        )
        .await
    }
}

#[derive(Default, Deserialize)]
pub struct HistoryQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub domain: Option<String>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub date_from: Option<String>,
    pub media_type: Option<String>,
}
impl HistoryQuery {
    fn bind(&self, sql: &str, actor: u64) -> Result<yoink_db::Query, DownloadError> {
        if !(0..=1_000_000).contains(&self.offset.unwrap_or(0))
            || !(1..=200).contains(&self.limit.unwrap_or(50))
            || self.search.as_ref().is_some_and(|v| v.len() > 1024)
            || self.domain.as_ref().is_some_and(|v| v.len() > 253)
            || self.status.as_deref().is_some_and(|s| {
                !["ok", "cached", "error", "queued", "running", "delivering"].contains(&s)
            })
            || self
                .media_type
                .as_deref()
                .is_some_and(|s| !["video", "audio", "gallery", "clip"].contains(&s))
        {
            return Err(DownloadError::InvalidInput);
        }
        let from = self
            .date_from
            .as_deref()
            .map(|v| {
                NaiveDate::parse_from_str(v, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|d| d.and_utc().timestamp_micros())
                    .ok_or(DownloadError::InvalidInput)
            })
            .transpose()?;
        Ok(query(sql)
            .bind(actor as i64)
            .bind(self.domain.clone())
            .bind(self.search.clone())
            .bind(self.status.clone())
            .bind(from)
            .bind(self.media_type.clone()))
    }
}
fn db(_: yoink_db::Error) -> DownloadError {
    DownloadError::Database
}
fn statement(suffix: &str) -> String {
    format!("{} {suffix}", include_str!("history.sql"))
}
impl UserSettingsService {
    pub async fn download_history(
        &self,
        actor: u64,
        filters: HistoryQuery,
    ) -> Result<Value, DownloadError> {
        self.authorize(actor).await?;
        let mut connection = self.database.connection().await.map_err(db)?;
        let tx = yoink_db::Transaction::new(
            &mut connection,
            turso::transaction::TransactionBehavior::Deferred,
        )
        .await
        .map_err(|_| DownloadError::Database)?;
        let total: i64 = filters
            .bind(&statement("SELECT count(*) AS total FROM filtered"), actor)?
            .fetch_one(&*tx)
            .await
            .map_err(db)?
            .try_get("total")
            .map_err(db)?;
        let rows = filters
            .bind(
                &statement(
                    "SELECT * FROM filtered ORDER BY created_at DESC,id DESC LIMIT ?7 OFFSET ?8",
                ),
                actor,
            )?
            .bind(filters.limit.unwrap_or(50))
            .bind(filters.offset.unwrap_or(0))
            .fetch_all(&*tx)
            .await
            .map_err(db)?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let mut item = serde_json::Map::new();
            let id: String = row.try_get("id").map_err(db)?;
            item.insert(
                "id".into(),
                id.parse::<i64>().map_or_else(|_| json!(id), |id| json!(id)),
            );
            for key in [
                "user_id",
                "file_size",
                "file_count",
                "group_id",
                "thread_id",
                "message_id",
            ] {
                item.insert(
                    key.into(),
                    json!(row.try_get::<Option<i64>, _>(key).map_err(db)?),
                );
            }
            for key in ["duration", "clip_start", "clip_end"] {
                item.insert(
                    key.into(),
                    json!(row.try_get::<Option<f64>, _>(key).map_err(db)?),
                );
            }
            for key in [
                "url",
                "domain",
                "title",
                "quality",
                "status",
                "error_msg",
                "group_title",
                "media_type",
            ] {
                item.insert(
                    key.into(),
                    json!(row.try_get::<Option<String>, _>(key).map_err(db)?),
                );
            }
            let timestamp: i64 = row.try_get("created_at").map_err(db)?;
            item.insert(
                "created_at".into(),
                json!(
                    DateTime::<Utc>::from_timestamp_micros(timestamp)
                        .ok_or(DownloadError::Database)?
                ),
            );
            items.push(Value::Object(item));
        }
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        drop(connection);
        self.authorize(actor).await?;
        Ok(
            json!({"items":items,"total":total,"offset":filters.offset.unwrap_or(0),"limit":filters.limit.unwrap_or(50)}),
        )
    }
    pub async fn download_domains(&self, actor: u64) -> Result<Value, DownloadError> {
        self.authorize(actor).await?;
        let rows = HistoryQuery::default()
            .bind(
                &statement(
                    "SELECT DISTINCT domain FROM filtered WHERE domain IS NOT NULL ORDER BY domain",
                ),
                actor,
            )?
            .fetch_all(&self.database)
            .await
            .map_err(db)?;
        let domains: Vec<String> = rows
            .iter()
            .map(|r| r.try_get("domain"))
            .collect::<Result<_, _>>()
            .map_err(db)?;
        self.authorize(actor).await?;
        Ok(json!({"domains":domains}))
    }
    pub async fn history_source(
        &self,
        actor: u64,
        id: &str,
    ) -> Result<Option<String>, DownloadError> {
        self.authorize(actor).await?;
        let row = HistoryQuery::default()
            .bind(&statement("SELECT url FROM filtered WHERE id=?7"), actor)?
            .bind(id)
            .fetch_optional(&self.database)
            .await
            .map_err(db)?;
        row.map(|r| r.try_get("url").map_err(db)).transpose()
    }
}
