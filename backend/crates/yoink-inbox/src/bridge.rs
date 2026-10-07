//! Item/star links use repository identity and user scope, not URL substring
//! joins. Synced links retain folder assignments; existing AI fields are seeded
//! only when NULL, never overwritten by a snapshot or manual item assignment.
use crate::{Result, normalize::repo_ref};
use serde_json::{Value, json};
use std::collections::HashMap;
use yoink_db::{Executor, query};
pub(crate) async fn link_all<E: Executor>(c: &E, user: i64, now: i64) -> Result<()> {
    let rows = query("SELECT id,full_name FROM inbox_gh_stars WHERE user_id=?")
        .bind(user)
        .fetch_all(c)
        .await?;
    let mut repos = HashMap::new();
    for r in rows {
        repos.insert(
            r.try_get::<String, _>("full_name")?.to_lowercase(),
            r.try_get::<i64, _>("id")?,
        );
    }
    let items = query("SELECT id,normalized_url,summary FROM inbox_items WHERE user_id=?")
        .bind(user)
        .fetch_all(c)
        .await?;
    for item in items {
        let url: String = item.try_get("normalized_url")?;
        let Some((owner, repo)) = repo_ref(&url) else {
            continue;
        };
        let Some(star) = repos.get(&format!("{owner}/{repo}").to_lowercase()) else {
            continue;
        };
        let id: i64 = item.try_get("id")?;
        query("INSERT INTO inbox_item_to_star(item_id,gh_star_id,synced_at) VALUES(?,?,?) ON CONFLICT(item_id,gh_star_id) DO UPDATE SET synced_at=excluded.synced_at").bind(id).bind(*star).bind(now).execute(c).await?;
        let labels=query("SELECT c.name FROM inbox_item_categories b JOIN inbox_categories c ON c.id=b.category_id WHERE b.item_id=? AND b.attached_by='ai' ORDER BY c.id").bind(id).fetch_all(c).await?.into_iter().map(|r|r.try_get::<String,_>("name")).collect::<std::result::Result<Vec<_>,_>>()?;
        query("UPDATE inbox_gh_stars SET ai_summary=coalesce(ai_summary,?),ai_labels=coalesce(ai_labels,?) WHERE id=? AND user_id=?").bind(item.try_get::<Option<String>,_>("summary")?).bind(if labels.is_empty(){None}else{Some(json!(labels))}).bind(*star).bind(user).execute(c).await?;
    }
    Ok(())
}
pub(crate) async fn star_rules<E: Executor>(c: &E, user: i64, now: i64) -> Result<usize> {
    use crate::rules::{Action, Condition, Facts, matches, string_op};
    let rules=query("SELECT conditions,actions FROM inbox_rules WHERE user_id=? AND enabled=1 AND trigger='star_synced' ORDER BY priority,id").bind(user).fetch_all(c).await?;
    let stars=query("SELECT id,full_name,html_url,language,ai_labels FROM inbox_gh_stars WHERE user_id=? ORDER BY id").bind(user).fetch_all(c).await?;
    let mut fired = 0;
    for star in stars {
        let id: i64 = star.try_get("id")?;
        let name: String = star.try_get("full_name")?;
        let url: String = star.try_get("html_url")?;
        let lang: Option<String> = star.try_get("language")?;
        let labels: Option<Value> = star.try_get("ai_labels")?;
        let original = labels.clone();
        let mut labels: Vec<String> = labels
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let items=query("SELECT i.id FROM inbox_item_to_star b JOIN inbox_items i ON i.id=b.item_id WHERE b.gh_star_id=? AND i.user_id=? ORDER BY i.id").bind(id).bind(user).fetch_all(c).await?;
        for rule in &rules {
            let cond: Option<Value> = rule.try_get("conditions")?;
            let actions: Option<Value> = rule.try_get("actions")?;
            let cond: Vec<Condition> = cond
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let actions: Vec<Action> = actions
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let facts = Facts {
                kind: "github_repo",
                status: "synced",
                url: &url,
                title: Some(&name),
                categories: &labels,
                language: lang.as_deref(),
            };
            if !cond.iter().all(|v| {
                if v.field == "language" {
                    string_op(lang.as_deref(), &v.op, &v.value)
                } else {
                    matches(v, &facts)
                }
            }) {
                continue;
            }
            fired += 1;
            for action in actions {
                match action.kind.as_str() {
                    "add_category" => {
                        if let Some(name) = action
                            .params
                            .get("category_name")
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                        {
                            if !labels
                                .iter()
                                .any(|s| s.to_lowercase() == name.to_lowercase())
                            {
                                labels.push(name.into());
                            }
                            for item in &items {
                                crate::store::attach_rule_category(
                                    c,
                                    user,
                                    item.try_get("id")?,
                                    name,
                                    now,
                                )
                                .await?;
                            }
                        }
                    }
                    "archive" => {
                        for item in &items {
                            query("UPDATE inbox_items SET status='archived',archived_at=?,modified_at=? WHERE id=? AND user_id=?").bind(now).bind(now).bind(item.try_get::<i64,_>("id")?).bind(user).execute(c).await?;
                        }
                    }
                    "set_status" => {
                        if let Some(status) = action.params.get("status").filter(|s| !s.is_empty())
                        {
                            for item in &items {
                                query("UPDATE inbox_items SET status=?,modified_at=? WHERE id=? AND user_id=?").bind(status).bind(now).bind(item.try_get::<i64,_>("id")?).bind(user).execute(c).await?;
                            }
                        }
                    }
                    _ => (),
                }
            }
        }
        if original.as_ref().is_some_and(|v| *v == json!(labels))
            || (original.is_none() && labels.is_empty())
        {
            continue;
        }
        query("UPDATE inbox_gh_stars SET ai_labels=? WHERE id=? AND user_id=?")
            .bind(if labels.is_empty() {
                None
            } else {
                Some(json!(labels))
            })
            .bind(id)
            .bind(user)
            .execute(c)
            .await?;
    }
    Ok(fired)
}
