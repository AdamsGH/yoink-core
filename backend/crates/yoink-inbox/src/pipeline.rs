use crate::{
    Error, Result,
    contracts::{Authorization, ContentProvider, Insight},
    github::Github,
    store::{Job, Store},
};
use serde_json::Value;
use yoink_db::query;
pub const DEFAULT_PROMPT: &str =
    "You are categorising an item for a personal inbox. Be concise and accurate.";
#[derive(Debug, Default)]
pub struct Classification {
    pub categories: Vec<Proposal>,
    pub summary: Option<String>,
}
#[derive(Debug)]
pub struct Proposal {
    pub name: String,
    pub new: bool,
    pub confidence: Option<f64>,
}
pub fn parse(raw: &str) -> Classification {
    let payload = serde_json::from_str::<Value>(raw).ok().or_else(|| {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        serde_json::from_str(&raw[start..=end]).ok()
    });
    let Some(payload) = payload.filter(Value::is_object) else {
        return Classification::default();
    };
    let categories = payload["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let name = v["name"].as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(Proposal {
                name: name.into(),
                new: v["kind"] == "new",
                confidence: v["confidence"].as_f64().filter(|f| f.is_finite()),
            })
        })
        .take(3)
        .collect();
    Classification {
        categories,
        summary: payload["summary"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    }
}
/// Single claimed job; coordinator supplies clock values after external work.
/// No DB lock is held during content, Insight or GitHub network calls.
pub async fn execute(
    store: &Store,
    job: &Job,
    auth: &dyn Authorization,
    insight: &dyn Insight,
    content: &dyn ContentProvider,
    github: &dyn Github,
    clock: impl Fn() -> i64,
) -> Result<bool> {
    auth.require(
        job.user_id,
        if job.kind == "stars_sync" {
            "inbox:gh_sync"
        } else {
            "inbox:ingest"
        },
    )
    .await?;
    match job.kind.as_str() {
        "stars_sync" => {
            let Some(token) = insight.github_token(job.user_id, false).await? else {
                return store.fail_job(job, clock(), "no_token", None).await;
            };
            let etag: Option<String> =
                query("SELECT etag FROM inbox_gh_sync_state WHERE user_id=?")
                    .bind(job.user_id)
                    .fetch_optional(&store.db)
                    .await?
                    .map(|r| r.try_get("etag"))
                    .transpose()?
                    .flatten();
            let snapshot = github.stars(&token, etag.as_deref()).await?;
            auth.require(job.user_id, "inbox:gh_sync").await?;
            let lists = if !snapshot.not_modified {
                if let Some(write_token) = insight.github_token(job.user_id, true).await? {
                    crate::lists::fetch(github, &write_token).await.ok()
                } else {
                    None
                }
            } else {
                None
            };
            // List pagination can outlive a grant or ban change as well.
            auth.require(job.user_id, "inbox:gh_sync").await?;
            store
                .apply_stars_with_lists(job, &snapshot, lists.as_deref(), clock())
                .await
        }
        "enrich" => {
            let id = job.item_id.ok_or(Error::Invalid)?;
            let item = store.item(job.user_id, id).await?;
            if item["status"] == "archived" || item["status"] == "failed" {
                return store.finish(job, clock(), None).await;
            }
            let data = content
                .extract(job.user_id, item["url"].as_str().ok_or(Error::Invalid)?)
                .await?;
            auth.require(job.user_id, "inbox:ingest").await?;
            store.apply_content(job, &data, clock()).await
        }
        "classify" => {
            auth.require(job.user_id, "inbox:classify").await?;
            let id = job.item_id.ok_or(Error::Invalid)?;
            let row = query(
                "SELECT title,url,content_text,status FROM inbox_items WHERE user_id=? AND id=?",
            )
            .bind(job.user_id)
            .bind(id)
            .fetch_optional(&store.db)
            .await?
            .ok_or(Error::NotFound)?;
            let status: String = row.try_get("status")?;
            if ["archived", "failed"].contains(&status.as_str()) {
                return store.finish(job, clock(), None).await;
            }
            let names=query("SELECT name FROM inbox_categories c WHERE owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=c.shared_with_team_id AND m.user_id=?) ORDER BY id").bind(job.user_id).bind(job.user_id).fetch_all(&store.db).await?.iter().map(|r|r.try_get::<String,_>("name")).collect::<std::result::Result<Vec<_>,_>>()?;
            let system: Option<String> =
                query("SELECT value FROM inbox_admin_settings WHERE key='classify_system_prompt'")
                    .fetch_optional(&store.db)
                    .await?
                    .map(|r| r.try_get("value"))
                    .transpose()?
                    .flatten();
            let hint = store.hint(job.user_id).await?;
            let language = insight.language(job.user_id).await?;
            let title: Option<String> = row.try_get("title")?;
            let url: String = row.try_get("url")?;
            let text: Option<String> = row.try_get("content_text")?;
            let excerpt: String = text.unwrap_or_default().chars().take(8000).collect();
            let prompt = format!(
                "{}\n\nThe user already has these categories:\n{}\nPick UP TO 3 categories that fit the item. You MAY propose AT MOST 1 new category if nothing existing fits well; mark it with kind=new.\nUser context: {}\nWrite all category names and the summary in {} language.\nOutput ONLY JSON: {{\"categories\":[{{\"name\":\"...\",\"kind\":\"existing or new\",\"confidence\":0.0}}],\"summary\":\"one to three sentences describing the item\",\"is_github_repo\":false}}\nTITLE: {}\nURL: {}\nEXTRACTED_CONTENT:\n{}",
                system
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(DEFAULT_PROMPT),
                names
                    .iter()
                    .map(|n| format!("- {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                hint.unwrap_or_default(),
                language.to_uppercase(),
                title.unwrap_or_else(|| "(none)".into()),
                url,
                excerpt
            );
            let raw = insight.complete(job.user_id, &prompt).await?;
            auth.require(job.user_id, "inbox:classify").await?;
            auth.require(job.user_id, "inbox:ingest").await?;
            store.apply_classification(job, &parse(&raw), clock()).await
        }
        _ => Err(Error::Invalid),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn salvage_and_caps() {
        let p = parse(
            "```json\n{\"categories\":[{\"name\":\" Rust \",\"kind\":\"new\",\"confidence\":0.8}],\"summary\":\" hello \"}\n```",
        );
        assert_eq!(p.categories[0].name, "Rust");
        assert_eq!(p.summary.as_deref(), Some("hello"));
    }
    #[test]
    fn invalid_empty() {
        assert!(parse("not JSON").categories.is_empty());
    }
}
