use chrono::{DateTime, Utc};
use yoink_db::{Row, query};

use crate::{Store, StoreError};

/// Resolver analytics. Timestamps use the shared signed UTC microsecond codec.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewMusicResolveLog {
    pub user_id: i64,
    pub source_platform: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub platforms_found: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicResolveLog {
    pub id: i64,
    pub resolve: NewMusicResolveLog,
}

impl Store {
    /// The user must already exist. One analytics row per successful resolution;
    /// this does not admit downloads or perform transport delivery.
    pub async fn insert_music_resolve_log(
        &self,
        resolve: &NewMusicResolveLog,
    ) -> Result<i64, StoreError> {
        if resolve.source_platform.chars().count() > 32
            || resolve
                .artist
                .as_ref()
                .is_some_and(|v| v.chars().count() > 256)
            || resolve
                .title
                .as_ref()
                .is_some_and(|v| v.chars().count() > 256)
        {
            return Err(StoreError::InvalidInput);
        }
        query(include_str!("../sql/music_resolve_log_insert.sql"))
            .bind(resolve.user_id)
            .bind(resolve.source_platform.clone())
            .bind(resolve.artist.clone())
            .bind(resolve.title.clone())
            .bind(resolve.platforms_found)
            .bind(resolve.created_at)
            .fetch_one(self.database())
            .await
            .map_err(|_| StoreError::Database)?
            .try_get("id")
            .map_err(|_| StoreError::Database)
    }

    /// Ascending identity pages retain deterministic ordering for equal timestamps.
    /// Pass None for the first page. Limit is bounded to 1..=1000.
    pub async fn music_resolve_logs(
        &self,
        user_id: i64,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<MusicResolveLog>, StoreError> {
        if !(1..=1000).contains(&limit) {
            return Err(StoreError::InvalidInput);
        }
        query(include_str!("../sql/music_resolve_log_list.sql"))
            .bind(user_id)
            .bind(after_id)
            .bind(limit)
            .fetch_all(self.database())
            .await
            .map_err(|_| StoreError::Database)?
            .iter()
            .map(decode)
            .collect()
    }
}

fn decode(row: &Row) -> Result<MusicResolveLog, StoreError> {
    let decode = || -> Result<MusicResolveLog, yoink_db::Error> {
        Ok(MusicResolveLog {
            id: row.try_get("id")?,
            resolve: NewMusicResolveLog {
                user_id: row.try_get("user_id")?,
                source_platform: row.try_get("source_platform")?,
                artist: row.try_get("artist")?,
                title: row.try_get("title")?,
                platforms_found: row.try_get("platforms_found")?,
                created_at: row.try_get("created_at")?,
            },
        })
    };
    decode().map_err(|_| StoreError::InvalidStoredData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn music_resolve_log_round_trip_pages_lengths_and_cascade() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = Store::open(
            &directory.path().join("music.db"),
            true,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        query("INSERT INTO users(id,created_at,updated_at) VALUES(?1,1,1)")
            .bind(9_007_199_254_740_993_i64)
            .execute(store.database())
            .await
            .unwrap();
        let resolve = NewMusicResolveLog {
            user_id: 9_007_199_254_740_993,
            source_platform: "spotify".into(),
            artist: Some("Исполнитель".into()),
            title: None,
            platforms_found: 3,
            created_at: DateTime::from_timestamp_micros(-1).unwrap(),
        };
        let first = store.insert_music_resolve_log(&resolve).await.unwrap();
        let second = store.insert_music_resolve_log(&resolve).await.unwrap();
        let logs = store
            .music_resolve_logs(resolve.user_id, None, 1)
            .await
            .unwrap();
        assert_eq!(
            logs,
            vec![MusicResolveLog {
                id: first,
                resolve: resolve.clone()
            }]
        );
        assert_eq!(
            store
                .music_resolve_logs(resolve.user_id, Some(first), 1)
                .await
                .unwrap()[0]
                .id,
            second
        );
        assert!(
            store
                .music_resolve_logs(resolve.user_id, None, 0)
                .await
                .is_err()
        );
        let mut invalid = resolve.clone();
        invalid.source_platform = "я".repeat(33);
        assert_eq!(
            store.insert_music_resolve_log(&invalid).await,
            Err(StoreError::InvalidInput)
        );
        invalid.source_platform = "я".repeat(32);
        invalid.artist = Some("я".repeat(256));
        store.insert_music_resolve_log(&invalid).await.unwrap();
        invalid.user_id = 999;
        assert!(store.insert_music_resolve_log(&invalid).await.is_err());
        store.close().await.unwrap();
        let store = Store::open(
            &directory.path().join("music.db"),
            false,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        store.verify_schema().await.unwrap();
        assert_eq!(
            store
                .music_resolve_logs(resolve.user_id, None, 1000)
                .await
                .unwrap()
                .len(),
            3
        );
        query("DELETE FROM users WHERE id=?1")
            .bind(resolve.user_id)
            .execute(store.database())
            .await
            .unwrap();
        assert!(
            store
                .music_resolve_logs(resolve.user_id, None, 1000)
                .await
                .unwrap()
                .is_empty()
        );
        store.verify_schema().await.unwrap();
    }
}
