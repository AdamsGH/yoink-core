use crate::*;
use chrono::{Datelike, Utc};
use serde::Serialize;
use yoink_db::query;
#[derive(Debug, Clone, Serialize)]
pub struct PlatformCount {
    pub platform: String,
    pub count: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct ArtistCount {
    pub artist: String,
    pub count: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct DayCount {
    pub date: String,
    pub count: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct MusicStats {
    pub total: i64,
    pub this_week: i64,
    pub today: i64,
    pub top_platforms: Vec<PlatformCount>,
    pub top_artists: Vec<ArtistCount>,
    pub by_day: Vec<DayCount>,
}
#[derive(Debug, Clone, Serialize)]
pub struct MusicActivity {
    pub plugin: &'static str,
    pub total: i64,
    pub last_at: Option<i64>,
}
impl NativeMusicPersistence {
    /// The caller must authorize the viewing principal and subject.
    pub async fn stats(&self, user_id: i64) -> Result<MusicStats, MusicError> {
        if user_id <= 0 {
            return Err(MusicError::InvalidInput);
        }
        let now = Utc::now();
        let today = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .ok_or(MusicError::Persistence)?
            .and_utc();
        let week =
            today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()));
        let r = query(include_str!("../sql/stats_counts.sql"))
            .bind(user_id)
            .bind(week.timestamp_micros())
            .bind(today.timestamp_micros())
            .fetch_one(&self.database)
            .await
            .map_err(|_| MusicError::Persistence)?;
        let total = r.try_get("total").map_err(|_| MusicError::Persistence)?;
        let this_week = r
            .try_get("this_week")
            .map_err(|_| MusicError::Persistence)?;
        let today = r.try_get("today").map_err(|_| MusicError::Persistence)?;
        let top_platforms = query(include_str!("../sql/stats_platforms.sql"))
            .bind(user_id)
            .fetch_all(&self.database)
            .await
            .map_err(|_| MusicError::Persistence)?
            .into_iter()
            .map(|r| {
                Ok(PlatformCount {
                    platform: r.try_get("platform").map_err(|_| MusicError::Persistence)?,
                    count: r.try_get("count").map_err(|_| MusicError::Persistence)?,
                })
            })
            .collect::<Result<_, MusicError>>()?;
        let top_artists = query(include_str!("../sql/stats_artists.sql"))
            .bind(user_id)
            .fetch_all(&self.database)
            .await
            .map_err(|_| MusicError::Persistence)?
            .into_iter()
            .map(|r| {
                Ok(ArtistCount {
                    artist: r.try_get("artist").map_err(|_| MusicError::Persistence)?,
                    count: r.try_get("count").map_err(|_| MusicError::Persistence)?,
                })
            })
            .collect::<Result<_, MusicError>>()?;
        let by_day = query(include_str!("../sql/stats_days.sql"))
            .bind(user_id)
            .bind((now - chrono::Duration::days(30)).timestamp_micros())
            .fetch_all(&self.database)
            .await
            .map_err(|_| MusicError::Persistence)?
            .into_iter()
            .map(|r| {
                Ok(DayCount {
                    date: r.try_get("date").map_err(|_| MusicError::Persistence)?,
                    count: r.try_get("count").map_err(|_| MusicError::Persistence)?,
                })
            })
            .collect::<Result<_, MusicError>>()?;
        Ok(MusicStats {
            total,
            this_week,
            today,
            top_platforms,
            top_artists,
            by_day,
        })
    }
    /// The caller must authorize the viewing principal and subject.
    pub async fn activity(&self, user_id: i64) -> Result<MusicActivity, MusicError> {
        if user_id <= 0 {
            return Err(MusicError::InvalidInput);
        }
        let r = query(include_str!("../sql/activity.sql"))
            .bind(user_id)
            .fetch_one(&self.database)
            .await
            .map_err(|_| MusicError::Persistence)?;
        Ok(MusicActivity {
            plugin: "music",
            total: r.try_get("total").map_err(|_| MusicError::Persistence)?,
            last_at: r.try_get("last_at").map_err(|_| MusicError::Persistence)?,
        })
    }
}

impl MusicService {
    pub async fn stats(
        &self,
        actor: MusicActor,
        persistence: &NativeMusicPersistence,
    ) -> Result<MusicStats, MusicError> {
        self.authorize(actor).await?;
        let result = persistence.stats(actor.user_id).await?;
        self.authorize(actor).await?;
        Ok(result)
    }

    pub async fn activity(
        &self,
        actor: MusicActor,
        persistence: &NativeMusicPersistence,
    ) -> Result<MusicActivity, MusicError> {
        self.authorize(actor).await?;
        let result = persistence.activity(actor.user_id).await?;
        self.authorize(actor).await?;
        Ok(result)
    }
}
