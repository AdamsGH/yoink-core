use crate::{
    Error, Result,
    normalize::{category_key, detect_kind, normalize_url},
};
use chrono::Utc;
use serde_json::{Value, json};
use yoink_db::{Database, Executor, query};
#[derive(Clone)]
pub struct Store {
    pub db: Database,
}
#[derive(Debug)]
pub struct IngestResult {
    pub item_id: i64,
    pub normalized_url: String,
    pub created: bool,
}
/// Persistent jobs use lease tokens to fence completions after crash/reclaim.
#[derive(Debug)]
pub struct Job {
    pub id: i64,
    pub user_id: i64,
    pub item_id: Option<i64>,
    pub kind: String,
    pub lease_token: String,
    pub attempts: i64,
}
impl Store {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
    pub async fn ingest(
        &self,
        user: i64,
        url: &str,
        source: &str,
        now: i64,
    ) -> Result<IngestResult> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let r = Self::ingest_on(&*c, user, url, source, now).await;
        match r {
            Ok(v) => {
                c.batch("COMMIT").await?;
                Ok(v)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
    /// Ingest on a caller-owned transaction. The caller must hold BEGIN IMMEDIATE
    /// and commit item, jobs, rules, bridges and its immutable command result
    /// together; this hook never acquires another connection or commits.
    pub async fn ingest_on<E: Executor>(
        c: &E,
        user: i64,
        url: &str,
        source: &str,
        now: i64,
    ) -> Result<IngestResult> {
        if !["bot", "web", "api", "extension"].contains(&source) {
            return Err(Error::Invalid);
        }
        let normalized = normalize_url(url)?;
        let kind = detect_kind(&normalized);
        let existing = query("SELECT id FROM inbox_items WHERE user_id=? AND normalized_url=?")
            .bind(user)
            .bind(&normalized)
            .fetch_optional(c)
            .await?;
        if let Some(row) = existing {
            return Ok(IngestResult {
                item_id: row.try_get("id")?,
                normalized_url: normalized.clone(),
                created: false,
            });
        }
        let row=query("INSERT INTO inbox_items(user_id,url,normalized_url,kind,source,status,created_at,modified_at) VALUES(?,?,?,?,?,'pending',?,?) RETURNING id").bind(user).bind(url.trim()).bind(&normalized).bind(kind).bind(source).bind(now).bind(now).fetch_one(c).await?;
        let id = row.try_get("id")?;
        enqueue(c, user, Some(id), "enrich", now).await?;
        run_rules(c, user, id, "item_ingested", now).await?;
        crate::bridge::link_all(c, user, now).await?;
        Ok(IngestResult {
            item_id: id,
            normalized_url: normalized.clone(),
            created: true,
        })
    }
    pub async fn enqueue(&self, user: i64, item: Option<i64>, kind: &str, now: i64) -> Result<()> {
        if !["enrich", "classify", "stars_sync"].contains(&kind) {
            return Err(Error::Invalid);
        }
        let c = self.db.connection().await?;
        if let Some(id) = item {
            query("SELECT id FROM inbox_items WHERE id=? AND user_id=?")
                .bind(id)
                .bind(user)
                .fetch_optional(&*c)
                .await?
                .ok_or(Error::NotFound)?;
        }
        enqueue(&*c, user, item, kind, now).await
    }
    pub async fn claim(&self, now: i64, lease: i64, token: &str) -> Result<Option<Job>> {
        if lease <= 0 || token.is_empty() {
            return Err(Error::Invalid);
        }
        let row=query("UPDATE inbox_jobs SET state='running',lease_until=?,lease_token=?,attempts=attempts+1 WHERE id=(SELECT id FROM inbox_jobs WHERE (state='ready' AND available_at<=?) OR (state='running' AND lease_until<=?) ORDER BY id LIMIT 1) RETURNING id,user_id,item_id,kind,attempts").bind(now.checked_add(lease).ok_or(Error::Invalid)?).bind(token).bind(now).bind(now).fetch_optional(&self.db).await?;
        row.map(|r| {
            Ok(Job {
                id: r.try_get("id")?,
                user_id: r.try_get("user_id")?,
                item_id: r.try_get("item_id")?,
                kind: r.try_get("kind")?,
                attempts: r.try_get("attempts")?,
                lease_token: token.into(),
            })
        })
        .transpose()
    }
    pub async fn finish(&self, job: &Job, now: i64, retry_at: Option<i64>) -> Result<bool> {
        Ok(query("UPDATE inbox_jobs SET state=?,available_at=?,lease_until=NULL,lease_token=NULL,last_error=? WHERE id=? AND state='running' AND lease_token=? AND lease_until>?").bind(if retry_at.is_some(){"ready"}else{"done"}).bind(retry_at.unwrap_or(now)).bind(retry_at.map(|_|"external operation failed")).bind(job.id).bind(&job.lease_token).bind(now).execute(&self.db).await?.rows_affected()==1)
    }
    pub async fn archive(&self, user: i64, id: i64, hard: bool, now: i64) -> Result<()> {
        let q = if hard {
            query("DELETE FROM inbox_items WHERE user_id=? AND id=?")
                .bind(user)
                .bind(id)
        } else {
            query("UPDATE inbox_items SET status='archived',archived_at=?,modified_at=? WHERE user_id=? AND id=?").bind(now).bind(now).bind(user).bind(id)
        };
        if q.execute(&self.db).await?.rows_affected() == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    pub async fn item(&self, user: i64, id: i64) -> Result<Value> {
        let row = query("SELECT * FROM inbox_items WHERE user_id=? AND id=?")
            .bind(user)
            .bind(id)
            .fetch_optional(&self.db)
            .await?
            .ok_or(Error::NotFound)?;
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), json!(row.try_get::<i64, _>("id")?));
        for key in ["url", "normalized_url", "kind", "source", "status"] {
            obj.insert(key.into(), json!(row.try_get::<String, _>(key)?));
        }
        for key in [
            "title",
            "summary",
            "crawl_status",
            "llm_status",
            "og_image_url",
            "favicon_url",
            "author",
            "publisher",
        ] {
            obj.insert(key.into(), json!(row.try_get::<Option<String>, _>(key)?));
        }
        for key in ["published_at", "created_at", "modified_at", "archived_at"] {
            let ts = row.try_get::<Option<i64>, _>(key)?;
            obj.insert(
                key.into(),
                json!(
                    ts.and_then(chrono::DateTime::<Utc>::from_timestamp_micros)
                        .map(|t| t.to_rfc3339())
                ),
            );
        }
        let rows=query("SELECT c.id,c.name,c.slug,c.color,b.attached_by,b.confidence FROM inbox_item_categories b JOIN inbox_categories c ON c.id=b.category_id WHERE b.item_id=? AND (c.owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=c.shared_with_team_id AND m.user_id=?)) ORDER BY c.id").bind(id).bind(user).bind(user).fetch_all(&self.db).await?;
        let mut cats = Vec::new();
        for r in rows {
            cats.push(json!({"id":r.try_get::<i64,_>("id")?,"name":r.try_get::<String,_>("name")?,"slug":r.try_get::<String,_>("slug")?,"color":r.try_get::<Option<String>,_>("color")?,"attached_by":r.try_get::<String,_>("attached_by")?,"confidence":r.try_get::<Option<f64>,_>("confidence")?}));
        }
        obj.insert("categories".into(), json!(cats));
        Ok(Value::Object(obj))
    }
    pub async fn assign_categories(&self, user: i64, id: i64, ids: &[i64], now: i64) -> Result<()> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let r = async {
 query("SELECT id FROM inbox_items WHERE id=? AND user_id=?").bind(id).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;
 for cat in ids {query("SELECT id FROM inbox_categories c WHERE c.id=? AND (c.owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=c.shared_with_team_id AND m.user_id=?))").bind(*cat).bind(user).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;}
 query("DELETE FROM inbox_item_categories WHERE item_id=?").bind(id).execute(&*c).await?;
 for cat in ids {query("INSERT INTO inbox_item_categories(item_id,category_id,attached_by,attached_by_user_id,attached_at) VALUES(?,?,'user',?,?) ON CONFLICT DO NOTHING").bind(id).bind(*cat).bind(user).bind(now).execute(&*c).await?;}
 Ok(()) }.await;
        match r {
            Ok(()) => {
                c.batch("COMMIT").await?;
                Ok(())
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
    pub async fn hint(&self, user: i64) -> Result<Option<String>> {
        Ok(
            query("SELECT classify_user_hint FROM inbox_user_settings WHERE user_id=?")
                .bind(user)
                .fetch_optional(&self.db)
                .await?
                .map(|r| r.try_get("classify_user_hint"))
                .transpose()?
                .flatten(),
        )
    }
    pub async fn set_hint(&self, user: i64, hint: Option<&str>, now: i64) -> Result<()> {
        crate::validation::bounded_optional(hint, 4000)?;
        query("INSERT INTO inbox_user_settings(user_id,classify_user_hint,created_at,updated_at) VALUES(?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET classify_user_hint=excluded.classify_user_hint,updated_at=excluded.updated_at").bind(user).bind(hint).bind(now).bind(now).execute(&self.db).await?;
        Ok(())
    }
}
async fn enqueue<E: Executor>(
    c: &E,
    user: i64,
    item: Option<i64>,
    kind: &str,
    now: i64,
) -> Result<()> {
    query("INSERT INTO inbox_jobs(user_id,item_id,kind,available_at,created_at) VALUES(?,?,?,?,?) ON CONFLICT DO NOTHING").bind(user).bind(item).bind(kind).bind(now).bind(now).execute(c).await?;
    Ok(())
}
async fn run_rules<E: Executor>(
    c: &E,
    user: i64,
    id: i64,
    trigger: &str,
    now: i64,
) -> Result<usize> {
    use crate::rules::{Action, Condition, Facts, matches};
    let rules=query("SELECT conditions,actions FROM inbox_rules WHERE user_id=? AND enabled=1 AND trigger=? ORDER BY priority,id").bind(user).bind(trigger).fetch_all(c).await?;
    let item = query("SELECT kind,status,url,title FROM inbox_items WHERE id=? AND user_id=?")
        .bind(id)
        .bind(user)
        .fetch_one(c)
        .await?;
    let rows=query("SELECT c.name FROM inbox_item_categories b JOIN inbox_categories c ON c.id=b.category_id WHERE b.item_id=?").bind(id).fetch_all(c).await?;
    let cats = rows
        .iter()
        .map(|r| r.try_get::<String, _>("name"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let kind: String = item.try_get("kind")?;
    let mut status: String = item.try_get("status")?;
    let url: String = item.try_get("url")?;
    let title: Option<String> = item.try_get("title")?;
    let mut fired = 0;
    for rule in rules {
        let conditions: Option<Value> = rule.try_get("conditions")?;
        let actions: Option<Value> = rule.try_get("actions")?;
        let conditions: Vec<Condition> = conditions
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let facts = Facts {
            kind: &kind,
            status: &status,
            url: &url,
            title: title.as_deref(),
            categories: &cats,
            language: None,
        };
        if !conditions.iter().all(|v| matches(v, &facts)) {
            continue;
        }
        fired += 1;
        let actions: Vec<Action> = actions
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        for a in actions {
            match a.kind.as_str() {
                "archive" => {
                    status = "archived".into();
                    query("UPDATE inbox_items SET status='archived',archived_at=?,modified_at=? WHERE id=? AND user_id=?").bind(now).bind(now).bind(id).bind(user).execute(c).await?;
                }
                "set_status" => {
                    if let Some(s) = a.params.get("status").filter(|s| !s.is_empty()) {
                        status = s.clone();
                        query("UPDATE inbox_items SET status=?,modified_at=? WHERE id=? AND user_id=?").bind(s).bind(now).bind(id).bind(user).execute(c).await?;
                    }
                }
                "add_category" => {
                    if let Some(name) = a
                        .params
                        .get("category_name")
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        let key = category_key(name);
                        let slug: String = key.chars().take(64).collect();
                        query("INSERT INTO inbox_categories(owner_user_id,name,normalized_name,slug,kind,created_at) VALUES(?,?,?,?,'ai',?) ON CONFLICT(owner_user_id,normalized_name) DO NOTHING").bind(user).bind(name).bind(&key).bind(slug).bind(now).execute(c).await?;
                        query("INSERT INTO inbox_item_categories(item_id,category_id,attached_by,attached_at) SELECT ?,id,'rule',? FROM inbox_categories WHERE owner_user_id=? AND normalized_name=? ON CONFLICT DO NOTHING").bind(id).bind(now).bind(user).bind(key).execute(c).await?;
                    }
                }
                _ => (),
            }
        }
    }
    Ok(fired)
}

impl Store {
    /// Atomic snapshot swap only after all REST pages validate. Existing AI fields survive.
    pub async fn apply_stars(
        &self,
        job: &Job,
        snapshot: &crate::github::StarSnapshot,
        now: i64,
    ) -> Result<bool> {
        self.apply_stars_with_lists(job, snapshot, None, now).await
    }
    pub async fn apply_stars_with_lists(
        &self,
        job: &Job,
        snapshot: &crate::github::StarSnapshot,
        lists: Option<&[crate::lists::ListSnapshot]>,
        now: i64,
    ) -> Result<bool> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let r = async {
 if !live(&*c,job,now).await?{return Ok(false)}
 if query("SELECT id FROM inbox_gh_mutations WHERE user_id=? AND state IN ('prepared','sending','unknown','confirmed')").bind(job.user_id).fetch_optional(&*c).await?.is_some(){return Err(Error::External)}
 if !snapshot.not_modified {
 for entry in &snapshot.rows {let repo=&entry["repo"];let id=repo["id"].as_i64().ok_or(Error::External)?;
 let ts=|v:&Value|v.as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t|t.timestamp_micros());
 query("INSERT INTO inbox_gh_stars(user_id,gh_repo_id,gh_node_id,full_name,owner_login,owner_avatar_url,description,language,topics,stargazers_count,html_url,homepage,archived,fork,starred_at,updated_at,can_unstar,last_synced_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(user_id,gh_repo_id) DO UPDATE SET gh_node_id=excluded.gh_node_id,full_name=excluded.full_name,owner_login=excluded.owner_login,owner_avatar_url=excluded.owner_avatar_url,description=excluded.description,language=excluded.language,topics=excluded.topics,stargazers_count=excluded.stargazers_count,html_url=excluded.html_url,homepage=excluded.homepage,archived=excluded.archived,fork=excluded.fork,starred_at=excluded.starred_at,updated_at=excluded.updated_at,can_unstar=excluded.can_unstar,last_synced_at=excluded.last_synced_at")
 .bind(job.user_id).bind(id).bind(repo["node_id"].as_str()).bind(repo["full_name"].as_str().ok_or(Error::External)?).bind(repo["owner"]["login"].as_str().ok_or(Error::External)?).bind(repo["owner"]["avatar_url"].as_str()).bind(repo["description"].as_str()).bind(repo["language"].as_str()).bind(repo.get("topics")).bind(repo["stargazers_count"].as_i64().unwrap_or(0)).bind(repo["html_url"].as_str().ok_or(Error::External)?).bind(repo["homepage"].as_str()).bind(repo["archived"].as_bool().unwrap_or(false)).bind(repo["fork"].as_bool().unwrap_or(false)).bind(ts(&entry["starred_at"])).bind(ts(&repo["updated_at"])).bind(snapshot.can_unstar).bind(now).execute(&*c).await?;
 }
 let ids:std::collections::HashSet<i64>=snapshot.rows.iter().filter_map(|r|r["repo"]["id"].as_i64()).collect();
 for old in query("SELECT id,gh_repo_id FROM inbox_gh_stars WHERE user_id=?").bind(job.user_id).fetch_all(&*c).await? {if !ids.contains(&old.try_get::<i64,_>("gh_repo_id")?) {query("DELETE FROM inbox_gh_stars WHERE user_id=? AND id=?").bind(job.user_id).bind(old.try_get::<i64,_>("id")?).execute(&*c).await?;}}
 }
 let count:i64=query("SELECT count(*) AS n FROM inbox_gh_stars WHERE user_id=?").bind(job.user_id).fetch_one(&*c).await?.try_get("n")?;
 query("INSERT INTO inbox_gh_sync_state(user_id,etag,last_synced_at,last_status,stars_count,created_at,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET etag=excluded.etag,last_synced_at=excluded.last_synced_at,last_status=excluded.last_status,last_error=NULL,stars_count=excluded.stars_count,updated_at=excluded.updated_at").bind(job.user_id).bind(&snapshot.etag).bind(now).bind(if snapshot.not_modified{"not_modified"}else{"ok"}).bind(count).bind(now).bind(now).execute(&*c).await?;
 crate::bridge::link_all(&*c,job.user_id,now).await?;
 if !snapshot.not_modified {crate::bridge::star_rules(&*c,job.user_id,now).await?;}
 if let Some(lists)=lists {crate::lists::apply(&*c,job.user_id,lists,now).await?;}
 done(&*c,job).await?;Ok(true)
 }.await;
        match r {
            Ok(v) => {
                c.batch("COMMIT").await?;
                Ok(v)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
    pub async fn apply_content(
        &self,
        job: &Job,
        content: &crate::contracts::Content,
        now: i64,
    ) -> Result<bool> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let r = async {
 if !live(&*c,job,now).await?{return Ok(false)}let id=job.item_id.ok_or(Error::Invalid)?;
 let changed=query("UPDATE inbox_items SET title=coalesce(?,title),summary=coalesce(summary,?),content_text=coalesce(?,content_text),og_image_url=coalesce(og_image_url,?),favicon_url=coalesce(favicon_url,?),author=coalesce(author,?),publisher=coalesce(publisher,?),published_at=coalesce(published_at,?),crawl_status=?,status=?,modified_at=? WHERE id=? AND user_id=? AND status NOT IN ('archived','failed')").bind(&content.title).bind(&content.summary_seed).bind(&content.content_text).bind(&content.og_image_url).bind(&content.favicon_url).bind(&content.author).bind(&content.publisher).bind(content.published_at).bind(&content.crawl_status).bind(if content.crawl_status.as_deref()==Some("failed"){"failed"}else{"enriched"}).bind(now).bind(id).bind(job.user_id).execute(&*c).await?.rows_affected();
 if let Some(repo)=content.github_repo_id{query("INSERT INTO inbox_item_to_star(item_id,gh_star_id,synced_at) SELECT ?,id,? FROM inbox_gh_stars WHERE user_id=? AND gh_repo_id=? ON CONFLICT(item_id,gh_star_id) DO UPDATE SET synced_at=excluded.synced_at").bind(id).bind(now).bind(job.user_id).bind(repo).execute(&*c).await?;}
 if changed>0 && content.crawl_status.as_deref()!=Some("failed"){enqueue(&*c,job.user_id,Some(id),"classify",now).await?;}
 done(&*c,job).await?;Ok(true)
 }.await;
        match r {
            Ok(v) => {
                c.batch("COMMIT").await?;
                Ok(v)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
    pub async fn apply_classification(
        &self,
        job: &Job,
        parsed: &crate::pipeline::Classification,
        now: i64,
    ) -> Result<bool> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let r = async {
 if !live(&*c,job,now).await?{return Ok(false)}let id=job.item_id.ok_or(Error::Invalid)?;
 let item=query("SELECT status FROM inbox_items WHERE id=? AND user_id=?").bind(id).bind(job.user_id).fetch_optional(&*c).await?;
 if item.is_none_or(|r|r.try_get::<String,_>("status").is_ok_and(|s|matches!(s.as_str(),"archived"|"failed"))){done(&*c,job).await?;return Ok(true)}
 query("DELETE FROM inbox_item_categories WHERE item_id=? AND attached_by='ai'").bind(id).execute(&*c).await?;
 let mut new_used=false;let mut notify_categories=Vec::new();
 for proposal in parsed.categories.iter().take(3){
 let key=category_key(&proposal.name);
 let existing=query("SELECT id FROM inbox_categories c WHERE normalized_name=? AND (owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=c.shared_with_team_id AND m.user_id=?)) ORDER BY CASE WHEN owner_user_id=? THEN 0 ELSE 1 END,id LIMIT 1").bind(&key).bind(job.user_id).bind(job.user_id).bind(job.user_id).fetch_optional(&*c).await?;
 let cat=if let Some(row)=existing{Some(row.try_get::<i64,_>("id")?)}else if proposal.new && !new_used && !key.is_empty(){
 let slug:String=key.chars().take(64).collect();
 let row=query("INSERT INTO inbox_categories(owner_user_id,name,normalized_name,slug,kind,created_at) VALUES(?,?,?,?,'ai',?) ON CONFLICT DO NOTHING RETURNING id").bind(job.user_id).bind(&proposal.name).bind(&key).bind(slug).bind(now).fetch_optional(&*c).await?;
 new_used=true;row.map(|r|r.try_get::<i64,_>("id")).transpose()?
 }else{None};
 if let Some(cat)=cat{notify_categories.push(query("SELECT name FROM inbox_categories WHERE id=?").bind(cat).fetch_one(&*c).await?.try_get::<String,_>("name")?);query("INSERT INTO inbox_item_categories(item_id,category_id,attached_by,confidence,attached_at) VALUES(?,?,'ai',?,?) ON CONFLICT DO NOTHING").bind(id).bind(cat).bind(proposal.confidence).bind(now).execute(&*c).await?;}
 }
 query("UPDATE inbox_items SET summary=coalesce(?,summary),status='classified',llm_status='success',modified_at=? WHERE id=? AND user_id=?").bind(&parsed.summary).bind(now).bind(id).bind(job.user_id).execute(&*c).await?;
 run_rules(&*c,job.user_id,id,"item_classified",now).await?;crate::bridge::link_all(&*c,job.user_id,now).await?;crate::notifications::admit(&*c,job,&notify_categories,now).await?;done(&*c,job).await?;Ok(true)
 }.await;
        match r {
            Ok(v) => {
                c.batch("COMMIT").await?;
                Ok(v)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
}
async fn live<E: Executor>(c: &E, job: &Job, now: i64) -> Result<bool> {
    Ok(query("SELECT id FROM inbox_jobs WHERE id=? AND user_id=? AND kind=? AND state='running' AND lease_token=? AND lease_until>?").bind(job.id).bind(job.user_id).bind(&job.kind).bind(&job.lease_token).bind(now).fetch_optional(c).await?.is_some())
}
async fn done<E: Executor>(c: &E, job: &Job) -> Result<()> {
    query("UPDATE inbox_jobs SET state='done',lease_token=NULL,lease_until=NULL WHERE id=? AND lease_token=?").bind(job.id).bind(&job.lease_token).execute(c).await?;
    Ok(())
}

impl Store {
    pub async fn fail_job(
        &self,
        job: &Job,
        now: i64,
        status: &str,
        retry_at: Option<i64>,
    ) -> Result<bool> {
        if ![
            "no_token",
            "auth_failed",
            "rate_limited",
            "error",
            "failed",
            "cancelled",
            "quota_exceeded",
            "insufficient_credits",
            "model_not_found",
            "invalid_config",
            "completion_failed",
        ]
        .contains(&status)
        {
            return Err(Error::Invalid);
        }
        // Cancellation must release/requeue the admission, never terminate it.
        // Require a future, caller-bounded retry time before touching the lease.
        if status == "cancelled" && retry_at.is_none_or(|at| at <= now) {
            return Err(Error::Invalid);
        }
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
 if !live(&*c,job,now).await?{return Ok(false)}
 if status=="cancelled" {
 // Restore only this live claim's consumed attempt. Stale/duplicate outcomes
 // cannot decrement attempts or overwrite the current holder's state.
 query("UPDATE inbox_jobs SET state='ready',attempts=attempts-1,available_at=?,lease_until=NULL,lease_token=NULL WHERE id=? AND state='running' AND lease_token=? AND lease_until>?").bind(retry_at).bind(job.id).bind(&job.lease_token).bind(now).execute(&*c).await?;
 // Preserve last_error, item/LLM status and stars-sync metadata: no failure occurred.
 return Ok(true)
 }
 if job.kind=="stars_sync" {
 query("INSERT INTO inbox_gh_sync_state(user_id,last_status,last_error,created_at,updated_at) VALUES(?,?,'external operation failed',?,?) ON CONFLICT(user_id) DO UPDATE SET last_status=excluded.last_status,last_error=excluded.last_error,updated_at=excluded.updated_at").bind(job.user_id).bind(status).bind(now).bind(now).execute(&*c).await?;
 }else if let Some(id)=job.item_id {
 if job.kind=="classify"{query("UPDATE inbox_items SET llm_status='failed',modified_at=? WHERE user_id=? AND id=? AND status<>'archived'").bind(now).bind(job.user_id).bind(id).execute(&*c).await?;}
 else if retry_at.is_none(){query("UPDATE inbox_items SET status='failed',crawl_status='failed',modified_at=? WHERE user_id=? AND id=? AND status<>'archived'").bind(now).bind(job.user_id).bind(id).execute(&*c).await?;}
 }
 query("UPDATE inbox_jobs SET state=?,available_at=?,lease_until=NULL,lease_token=NULL,last_error='external operation failed' WHERE id=? AND lease_token=?").bind(if retry_at.is_some(){"ready"}else{"failed"}).bind(retry_at.unwrap_or(now)).bind(job.id).bind(&job.lease_token).execute(&*c).await?;Ok(true)
 }.await;
        match result {
            Ok(v) => {
                c.batch("COMMIT").await?;
                Ok(v)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
}

impl Store {
    pub async fn test_rule(&self, user: i64, rule_id: i64, item_id: i64) -> Result<Value> {
        use crate::rules::{Condition, Facts, matches};
        let rule = self
            .catalogue_one(user, crate::catalog::Catalog::Rule, rule_id)
            .await?;
        let item = self.item(user, item_id).await?;
        let names: Vec<String> = item["categories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .collect();
        let facts = Facts {
            kind: item["kind"].as_str().unwrap_or(""),
            status: item["status"].as_str().unwrap_or(""),
            url: item["url"].as_str().unwrap_or(""),
            title: item["title"].as_str(),
            categories: &names,
            language: None,
        };
        let mut results = Vec::new();
        for v in rule["conditions"].as_array().into_iter().flatten() {
            if let Ok(cond) = serde_json::from_value::<Condition>(v.clone()) {
                let mut v = v.clone();
                v["passed"] = json!(matches(&cond, &facts));
                results.push(v);
            }
        }
        let matched = results.iter().all(|v| v["passed"] == true);
        Ok(
            json!({"rule_id":rule_id,"matched":matched,"conditions_result":results,"actions_would_fire":if matched {rule["actions"].as_array().cloned().unwrap_or_default()}else{vec![]}}),
        )
    }
    /// Matches the legacy sweep: all enabled rules of the selected rule's trigger,
    /// not only the selected rule, over at most 500 non-archived items.
    pub async fn sweep_rule(&self, user: i64, rule_id: i64, now: i64) -> Result<Value> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
 let row=query("SELECT trigger FROM inbox_rules WHERE user_id=? AND id=?").bind(user).bind(rule_id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let trigger:String=row.try_get("trigger")?;
 let rows=query("SELECT id FROM inbox_items WHERE user_id=? AND archived_at IS NULL ORDER BY created_at DESC,id DESC LIMIT 500").bind(user).fetch_all(&*c).await?;let mut fired=0;
 for row in &rows {fired+=run_rules(&*c,user,row.try_get("id")?,&trigger,now).await?;}Ok(json!({"fired":fired,"scanned":rows.len()}))
 }.await;
        match result {
            Ok(v) => {
                c.batch("COMMIT").await?;
                Ok(v)
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                Err(e)
            }
        }
    }
}

/// Full persisted model for sql/durable_jobs.sql, separate from the claimed view.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct JobRecord {
    pub id: i64,
    pub user_id: i64,
    pub item_id: Option<i64>,
    pub kind: String,
    pub state: String,
    pub attempts: i64,
    pub available_at: i64,
    pub lease_until: Option<i64>,
    pub lease_token: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
}

pub(crate) async fn attach_rule_category<E: Executor>(
    c: &E,
    user: i64,
    item: i64,
    name: &str,
    now: i64,
) -> Result<()> {
    let key = category_key(name);
    let slug: String = key.chars().take(64).collect();
    query("INSERT INTO inbox_categories(owner_user_id,name,normalized_name,slug,kind,created_at) VALUES(?,?,?,?,'ai',?) ON CONFLICT(owner_user_id,normalized_name) DO NOTHING").bind(user).bind(name).bind(&key).bind(slug).bind(now).execute(c).await?;
    query("INSERT INTO inbox_item_categories(item_id,category_id,attached_by,attached_at) SELECT ?,id,'rule',? FROM inbox_categories WHERE owner_user_id=? AND normalized_name=? ON CONFLICT DO NOTHING").bind(item).bind(now).bind(user).bind(key).execute(c).await?;
    Ok(())
}
