use crate::{Error, Result, store::Store};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use yoink_db::query;
#[derive(Default, Deserialize)]
pub struct Filters {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub category_id: Option<i64>,
    #[serde(default)]
    pub uncategorized: bool,
    pub search: Option<String>,
}
pub fn encode_cursor(timestamp: i64, id: i64) -> Result<String> {
    let t = chrono::DateTime::from_timestamp_micros(timestamp).ok_or(Error::Invalid)?;
    Ok(URL_SAFE_NO_PAD.encode(format!("{}|{id}", t.to_rfc3339())))
}
pub fn decode_cursor(raw: Option<&str>) -> Result<Option<(i64, i64)>> {
    let Some(raw) = raw else { return Ok(None) };
    let data = URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .map_err(|_| Error::Invalid)?;
    let text = std::str::from_utf8(&data).map_err(|_| Error::Invalid)?;
    let (t, id) = text.split_once('|').ok_or(Error::Invalid)?;
    let t = chrono::DateTime::parse_from_rfc3339(t)
        .map_err(|_| Error::Invalid)?
        .timestamp_micros();
    let id = id.parse().map_err(|_| Error::Invalid)?;
    Ok(Some((t, id)))
}
impl Store {
    pub async fn feed(&self, user: i64, f: &Filters) -> Result<Value> {
        let limit = f.limit.unwrap_or(30);
        if !(1..=100).contains(&limit)
            || f.search
                .as_ref()
                .is_some_and(|s| s.chars().count() < 2 || s.chars().count() > 128)
        {
            return Err(Error::Invalid);
        }
        crate::search::validate(f.search.as_deref())?;
        let cursor = decode_cursor(f.cursor.as_deref())?;
        let mut sql = String::from("SELECT i.* FROM inbox_items i WHERE i.user_id=?");
        if f.status.as_deref().is_some_and(|s| !s.is_empty()) {
            sql.push_str(" AND i.status=?")
        } else {
            sql.push_str(" AND i.archived_at IS NULL")
        }
        if f.kind.as_deref().is_some_and(|s| !s.is_empty()) {
            sql.push_str(" AND i.kind=?")
        }
        if f.category_id.is_some() {
            sql.push_str(" AND EXISTS(SELECT 1 FROM inbox_item_categories b JOIN inbox_categories c ON c.id=b.category_id WHERE b.item_id=i.id AND c.id=? AND (c.owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=c.shared_with_team_id AND m.user_id=?)))")
        } else if f.uncategorized {
            sql.push_str(
                " AND NOT EXISTS(SELECT 1 FROM inbox_item_categories b WHERE b.item_id=i.id)",
            )
        }
        if cursor.is_some() {
            sql.push_str(" AND (i.created_at<? OR (i.created_at=? AND i.id<?))")
        }
        sql.push_str(" ORDER BY i.created_at DESC,i.id DESC LIMIT ?");
        let mut q = query(&sql).bind(user);
        if let Some(s) = f.status.as_ref().filter(|s| !s.is_empty()) {
            q = q.bind(s)
        }
        if let Some(k) = f.kind.as_ref().filter(|s| !s.is_empty()) {
            q = q.bind(k)
        }
        if let Some(cat) = f.category_id {
            q = q.bind(cat).bind(user).bind(user)
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
            .fetch_all(&self.db)
            .await?;
        let rows = crate::search::filter(rows, f.search.as_deref(), &["title", "summary", "url"])?;
        let mut items = Vec::new();
        let mut next = None;
        for r in rows.iter().take(limit) {
            let id = r.try_get("id")?;
            items.push(self.item(user, id).await?);
        }
        if rows.len() > limit {
            let last = &rows[limit - 1];
            next = Some(encode_cursor(
                last.try_get("created_at")?,
                last.try_get("id")?,
            )?)
        }
        Ok(json!({"items":items,"next_cursor":next}))
    }
    pub async fn categories(&self, user: i64) -> Result<Value> {
        let rows=query("SELECT c.*,(SELECT count(*) FROM inbox_item_categories b JOIN inbox_items i ON i.id=b.item_id WHERE b.category_id=c.id AND i.user_id=? AND i.archived_at IS NULL) AS item_count FROM inbox_categories c WHERE c.owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=c.shared_with_team_id AND m.user_id=?) ORDER BY c.name,c.id").bind(user).bind(user).bind(user).fetch_all(&self.db).await?;
        let mut all = Vec::new();
        for r in rows {
            let mut v = serde_json::Map::new();
            for key in ["id", "owner_user_id", "item_count"] {
                v.insert(key.into(), json!(r.try_get::<i64, _>(key)?));
            }
            for key in ["parent_id", "shared_with_team_id"] {
                v.insert(key.into(), json!(r.try_get::<Option<i64>, _>(key)?));
            }
            for key in ["name", "slug", "kind"] {
                v.insert(key.into(), json!(r.try_get::<String, _>(key)?));
            }
            for key in ["icon", "color", "description"] {
                v.insert(key.into(), json!(r.try_get::<Option<String>, _>(key)?));
            }
            all.push(Value::Object(v));
        }
        Ok(json!(all))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor() {
        let c = encode_cursor(123456789, 42).unwrap();
        assert_eq!(decode_cursor(Some(&c)).unwrap(), Some((123456789, 42)));
        assert!(decode_cursor(Some("invalid")).is_err());
    }
}
