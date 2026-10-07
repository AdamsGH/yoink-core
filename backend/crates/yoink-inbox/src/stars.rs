use crate::{
    Error, Result,
    feed::{decode_cursor, encode_cursor},
    store::Store,
};
use serde::Deserialize;
use serde_json::{Value, json};
use yoink_db::query;
#[derive(Default, Deserialize)]
pub struct StarFilters {
    pub cursor: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub language: Option<String>,
    pub search: Option<String>,
    pub folder_id: Option<i64>,
    pub sort: Option<String>,
}
impl Store {
    pub async fn star_languages(&self, user: i64) -> Result<Value> {
        let rows=query("SELECT DISTINCT language FROM inbox_gh_stars WHERE user_id=? AND language IS NOT NULL ORDER BY language").bind(user).fetch_all(&self.db).await?;
        Ok(json!(
            rows.iter()
                .map(|r| r.try_get::<String, _>("language"))
                .collect::<std::result::Result<Vec<_>, _>>()?
        ))
    }
    pub async fn stars(&self, user: i64, f: &StarFilters) -> Result<Value> {
        let limit = f.limit.unwrap_or(30);
        if !(1..=100).contains(&limit) {
            return Err(Error::Invalid);
        }
        crate::search::validate(f.search.as_deref())?;
        let sort = f
            .sort
            .as_deref()
            .filter(|s| ["starred_at", "stargazers_count", "name"].contains(s))
            .unwrap_or("starred_at");
        let cursor = if sort == "starred_at" {
            decode_cursor(f.cursor.as_deref())?
        } else {
            None
        };
        let order = match sort {
            "starred_at" => "s.starred_at DESC,s.id DESC",
            "stargazers_count" => "s.stargazers_count DESC,s.id DESC",
            "name" => "s.full_name ASC,s.id DESC",
            _ => return Err(Error::Invalid),
        };
        let mut sql = String::from("SELECT s.* FROM inbox_gh_stars s WHERE s.user_id=?");
        if f.language.as_deref().is_some_and(|s| !s.is_empty()) {
            sql.push_str(" AND s.language=?")
        }
        if f.folder_id == Some(0) {
            sql.push_str(" AND NOT EXISTS(SELECT 1 FROM inbox_gh_folder_members m JOIN inbox_gh_folders f ON f.id=m.folder_id WHERE m.gh_star_id=s.id AND f.user_id=s.user_id)");
        } else if f.folder_id.is_some() {
            sql.push_str(" AND EXISTS(SELECT 1 FROM inbox_gh_folder_members m JOIN inbox_gh_folders f ON f.id=m.folder_id WHERE m.gh_star_id=s.id AND f.id=? AND f.user_id=?)")
        }
        if cursor.is_some() {
            sql.push_str(" AND (s.starred_at<? OR (s.starred_at=? AND s.id<?))")
        }
        sql.push_str(&format!(" ORDER BY {order} LIMIT ? OFFSET ?"));
        let mut q = query(&sql).bind(user);
        if let Some(v) = f.language.as_ref().filter(|s| !s.is_empty()) {
            q = q.bind(v)
        }
        if let Some(id) = f.folder_id.filter(|id| *id != 0) {
            q = q.bind(id).bind(user)
        }
        if let Some((ts, id)) = cursor {
            q = q.bind(ts).bind(ts).bind(id)
        }
        let rows = q
            .bind(if f.search.is_some() {
                -1_i64
            } else {
                (limit + 1) as i64
            })
            .bind(if sort == "starred_at" || f.search.is_some() {
                0
            } else {
                f.offset.unwrap_or(0)
            })
            .fetch_all(&self.db)
            .await?;
        let rows = crate::search::filter(rows, f.search.as_deref(), &["full_name", "description"])?;
        let rows = if f.search.is_some() && sort != "starred_at" {
            rows.into_iter().skip(f.offset.unwrap_or(0)).collect()
        } else {
            rows
        };
        let mut all = Vec::new();
        let mut next = None;
        for r in rows.iter().take(limit) {
            let mut v = serde_json::Map::new();
            for k in ["id", "gh_repo_id", "stargazers_count"] {
                v.insert(k.into(), json!(r.try_get::<i64, _>(k)?));
            }
            for k in ["full_name", "owner_login", "html_url"] {
                v.insert(k.into(), json!(r.try_get::<String, _>(k)?));
            }
            for k in [
                "owner_avatar_url",
                "description",
                "language",
                "homepage",
                "ai_summary",
            ] {
                v.insert(k.into(), json!(r.try_get::<Option<String>, _>(k)?));
            }
            for k in ["topics", "ai_labels"] {
                v.insert(
                    k.into(),
                    r.try_get::<Option<Value>, _>(k)?.unwrap_or(Value::Null),
                );
            }
            for k in ["archived", "fork", "can_unstar"] {
                v.insert(k.into(), json!(r.try_get::<bool, _>(k)?));
            }
            for k in ["starred_at", "updated_at", "last_synced_at"] {
                let ts = r.try_get::<Option<i64>, _>(k)?;
                v.insert(
                    k.into(),
                    json!(
                        ts.and_then(chrono::DateTime::from_timestamp_micros)
                            .map(|t| t.to_rfc3339())
                    ),
                );
            }
            let ids=query("SELECT m.folder_id FROM inbox_gh_folder_members m JOIN inbox_gh_folders f ON f.id=m.folder_id WHERE m.gh_star_id=? AND f.user_id=? ORDER BY m.folder_id").bind(r.try_get::<i64,_>("id")?).bind(user).fetch_all(&self.db).await?.iter().map(|r|r.try_get::<i64,_>("folder_id")).collect::<std::result::Result<Vec<_>,_>>()?;
            v.insert("folder_ids".into(), json!(ids));
            all.push(Value::Object(v));
        }
        if rows.len() > limit && sort != "starred_at" {
            next = Some((f.offset.unwrap_or(0) + limit).to_string());
        } else if rows.len() > limit {
            let r = &rows[limit - 1];
            if let Some(ts) = r.try_get::<Option<i64>, _>("starred_at")? {
                next = Some(encode_cursor(ts, r.try_get("id")?)?)
            }
        }
        let state =
            query("SELECT last_status,last_synced_at FROM inbox_gh_sync_state WHERE user_id=?")
                .bind(user)
                .fetch_optional(&self.db)
                .await?;
        let (status, ts) = if let Some(r) = state {
            (
                r.try_get::<Option<String>, _>("last_status")?,
                r.try_get::<Option<i64>, _>("last_synced_at")?,
            )
        } else {
            (None, None)
        };
        Ok(
            json!({"items":all,"next_cursor":next,"sync_status":status,"last_synced_at":ts.and_then(chrono::DateTime::from_timestamp_micros).map(|t|t.to_rfc3339())}),
        )
    }
}
