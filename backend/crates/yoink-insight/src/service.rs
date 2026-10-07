use crate::{
    Error,
    content::{ContentSource, normalize, video_id},
    invalid,
    prompts::{self, Alias, AliasInput},
    provider::{Completion, Route, Shape},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_access::{FeatureSpec, Grant, Principal, Role, effective_grant};
use yoink_db::{Database, Executor, query};
#[derive(Clone)]
pub struct Config {
    pub owner_id: Option<i64>,
    pub gateway_base: String,
    pub gateway_key: String,
    pub openrouter_base: String,
    pub openrouter_key: String,
    pub mode: String,
    pub model: String,
    pub gemini_key: String,
    pub gemini_model: String,
    pub summary_limit: i64,
    pub tldr_limit: i64,
    pub max_chars: usize,
    pub transcript_languages: Vec<String>,
    pub search_model: String,
    pub request_timeout: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            owner_id: None,
            gateway_base: "http://0.0.0.0:8317/v1".into(),
            gateway_key: String::new(),
            openrouter_base: "https://openrouter.ai/api/v1".into(),
            openrouter_key: String::new(),
            mode: "auto".into(),
            model: String::new(),
            gemini_key: String::new(),
            gemini_model: "gemini-2.0-flash".into(),
            summary_limit: 50,
            tldr_limit: 20,
            max_chars: 40000,
            transcript_languages: vec!["en".into(), "ru".into()],
            search_model: String::new(),
            request_timeout: Duration::from_secs(120),
        }
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InsightConfig([REDACTED])")
    }
}
#[derive(Clone)]
pub struct Insight {
    pub db: Database,
    pub config: Config,
    pub content: Option<Arc<dyn ContentSource>>,
    pub completion: Arc<dyn Completion>,
    pub research: Option<Arc<dyn crate::research::Research>>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Request {
    pub command: String,
    pub url: String,
    pub alias: Option<String>,
    pub focus: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub result: String,
    pub cached: bool,
    pub route: String,
    pub alias: Option<String>,
}
impl Insight {
    pub fn new(
        db: Database,
        config: Config,
        content: Arc<dyn ContentSource>,
        completion: Arc<dyn Completion>,
    ) -> Result<Self, Error> {
        let mut insight = Self::completion_only(db, config, completion)?;
        insight.content = Some(content);
        Ok(insight)
    }
    /// Completion, credential and language dependency without a content worker.
    /// Content-dependent operations fail explicitly until a real source is installed.
    pub fn completion_only(
        db: Database,
        config: Config,
        completion: Arc<dyn Completion>,
    ) -> Result<Self, Error> {
        if config.request_timeout.is_zero()
            || config.request_timeout > Duration::from_secs(300)
            || !(100..=200000).contains(&config.max_chars)
            || config.summary_limit < 0
            || config.tldr_limit < 0
            || !matches!(config.mode.as_str(), "auto" | "a2a" | "openrouter")
        {
            return Err(invalid());
        }
        Ok(Self {
            db,
            config,
            content: None,
            completion,
            research: None,
        })
    }
    /// Install the coordinator-owned real content source on this same service.
    pub fn with_content(mut self, content: Arc<dyn ContentSource>) -> Self {
        self.content = Some(content);
        self
    }
    pub fn with_research(mut self, research: Arc<dyn crate::research::Research>) -> Self {
        self.research = Some(research);
        self
    }
    /// Authorization runs against the held native connection. Callers must retain
    /// that guard through writes; no JWT roles or stale provider grants are trusted.
    pub(crate) async fn authorize<E: Executor>(
        &self,
        e: &E,
        uid: i64,
        feature: Option<&str>,
        admin: bool,
    ) -> Result<(), Error> {
        self.authorize_inner(e, uid, feature, admin, true).await
    }
    pub(crate) async fn authorize_gateway<E: Executor>(
        &self,
        e: &E,
        uid: i64,
        feature: &str,
    ) -> Result<(), Error> {
        self.authorize_inner(e, uid, Some(feature), false, false)
            .await
    }
    async fn authorize_inner<E: Executor>(
        &self,
        e: &E,
        uid: i64,
        feature: Option<&str>,
        admin: bool,
        provider_grants: bool,
    ) -> Result<(), Error> {
        let row = query("SELECT role,ban_until FROM users WHERE id=?")
            .bind(uid)
            .fetch_optional(e)
            .await?
            .ok_or(Error::Code("access_denied"))?;
        let mut role: Role = row
            .try_get::<String, _>("role")?
            .parse()
            .map_err(|_| Error::Code("access_denied"))?;
        let until: Option<DateTime<Utc>> = row.try_get("ban_until")?;
        if uid == self.config.owner_id.unwrap_or(0) && role != Role::Banned {
            role = Role::Owner
        }
        let p = Principal {
            user_id: uid,
            role,
            ban_until: until,
        };
        let now = Utc::now();
        if uid <= 0 || p.is_blocked(now) || (admin && role < Role::Admin) {
            return Err(Error::Code("access_denied"));
        }
        if let Some(feature) = feature {
            let (plugin, feature) = feature.split_once(':').unwrap_or(("insight", feature));
            let mut grants =
                query("SELECT plugin,feature,expires_at FROM user_permissions WHERE user_id=?")
                    .bind(uid)
                    .fetch_all(e)
                    .await?
                    .iter()
                    .map(|r| {
                        Ok(Grant {
                            plugin: r.try_get("plugin")?,
                            feature: r.try_get("feature")?,
                            expires_at: r.try_get("expires_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, yoink_db::Error>>()?;
            let raw = self.setting(e, "tag_map", "{}").await?;
            if let Ok(map) =
                serde_json::from_str::<std::collections::BTreeMap<String, Vec<String>>>(&raw)
                && !map.is_empty()
                && map.iter().all(|(tag, keys)| {
                    !tag.trim().is_empty()
                        && !tag.chars().any(char::is_control)
                        && keys.iter().collect::<std::collections::BTreeSet<_>>().len()
                            == keys.len()
                        && keys.iter().all(|key| {
                            key.split_once(':').is_some_and(|(p, f)| {
                                yoink_identity::registered_features()
                                    .iter()
                                    .any(|s| s.plugin == p && s.feature == f)
                            })
                        })
                })
            {
                let tags = query(include_str!("../sql/active_tags.sql"))
                    .bind(uid)
                    .fetch_all(e)
                    .await?
                    .iter()
                    .map(|r| r.try_get::<String, _>(0))
                    .collect::<Result<Vec<_>, _>>()?;
                let suppressed = query(
                    "SELECT plugin,feature FROM membership_feature_suppressions WHERE user_id=?",
                )
                .bind(uid)
                .fetch_all(e)
                .await?
                .iter()
                .map(|r| Ok((r.try_get::<String, _>(0)?, r.try_get::<String, _>(1)?)))
                .collect::<Result<Vec<_>, yoink_db::Error>>()?;
                for (tag, keys) in map {
                    if tags.contains(&tag) {
                        for key in keys {
                            if let Some((p, f)) = key.split_once(':')
                                && yoink_identity::registered_features()
                                    .iter()
                                    .any(|s| s.plugin == p && s.feature == f)
                                && !suppressed.contains(&(p.into(), f.into()))
                            {
                                grants.push(Grant {
                                    plugin: p.into(),
                                    feature: f.into(),
                                    expires_at: None,
                                });
                            }
                        }
                    }
                }
            }
            let spec = yoink_identity::registered_features()
                .into_iter()
                .find(|s| s.plugin == plugin && s.feature == feature)
                .unwrap_or(FeatureSpec {
                    plugin: plugin.into(),
                    feature: feature.into(),
                    default_min_role: None,
                });
            if effective_grant(&p, &spec, &grants, &[], now).is_none() {
                // Tested BYOK is a provider grant only while the global tumbler is on.
                let byok=provider_grants&&plugin=="insight"&&feature=="tldr"&&self.byok_enabled(e).await?&&query("SELECT 1 FROM insight_user_byok WHERE user_id=? AND tested_at IS NOT NULL AND test_error IS NULL").bind(uid).fetch_optional(e).await?.is_some();
                if !byok {
                    return Err(Error::Code("access_denied"));
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn setting<E: Executor>(
        &self,
        e: &E,
        key: &str,
        default: &str,
    ) -> Result<String, Error> {
        Ok(query("SELECT value FROM bot_settings WHERE key=?")
            .bind(key)
            .fetch_optional(e)
            .await?
            .map(|r| r.try_get::<Option<String>, _>("value"))
            .transpose()?
            .flatten()
            .unwrap_or_else(|| default.into()))
    }
    pub(crate) async fn byok_enabled<E: Executor>(&self, e: &E) -> Result<bool, Error> {
        Ok(matches!(
            self.setting(e, "insight_byok_enabled", "false")
                .await?
                .to_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        ))
    }
    pub(crate) async fn put_setting<E: Executor>(
        &self,
        e: &E,
        key: &str,
        value: &str,
    ) -> Result<(), Error> {
        query("INSERT INTO bot_settings(key,value,updated_at) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at").bind(key).bind(value).bind(Utc::now()).execute(e).await?;
        Ok(())
    }
    pub async fn aliases(&self, uid: i64) -> Result<Vec<Alias>, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        self.load_aliases(&*c, uid).await
    }
    async fn load_aliases<E: Executor>(&self, e: &E, uid: i64) -> Result<Vec<Alias>, Error> {
        query("SELECT * FROM insight_tldr_aliases WHERE user_id=? ORDER BY id")
            .bind(uid)
            .fetch_all(e)
            .await?
            .iter()
            .map(|r| {
                Ok(Alias {
                    id: r.try_get("id")?,
                    aliases: r.try_get("aliases")?,
                    prompt: r.try_get("prompt")?,
                    domains: r.try_get("domains")?,
                    target_alias: r.try_get("target_alias")?,
                    created_at: r.try_get("created_at")?,
                })
            })
            .collect()
    }
    pub async fn save_alias(
        &self,
        uid: i64,
        id: Option<i64>,
        a: AliasInput,
    ) -> Result<Alias, Error> {
        a.validate()?;
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, Some("tldr"), false).await?;
        let rows = self.load_aliases(&*c, uid).await?;
        if id.is_some_and(|id| !rows.iter().any(|r| r.id == id)) {
            return Err(Error::Code("not_found"));
        }
        if id.is_none() && rows.len() >= 20 {
            return Err(Error::Code("alias_limit"));
        }
        let keys = prompts::csv(a.aliases.as_deref());
        if rows.iter().filter(|r| Some(r.id) != id).any(|r| {
            prompts::csv(r.aliases.as_deref())
                .iter()
                .any(|k| keys.contains(k))
        }) {
            return Err(Error::Code("alias_conflict"));
        }
        let id = if let Some(id) = id {
            query("UPDATE insight_tldr_aliases SET aliases=?,prompt=?,domains=?,target_alias=? WHERE id=? AND user_id=?").bind(a.aliases).bind(a.prompt).bind(a.domains).bind(a.target_alias).bind(id).bind(uid).execute(&*c).await?;
            id
        } else {
            query("INSERT INTO insight_tldr_aliases(user_id,aliases,prompt,domains,target_alias) VALUES(?,?,?,?,?)").bind(uid).bind(a.aliases).bind(a.prompt).bind(a.domains).bind(a.target_alias).execute(&*c).await?;
            query("SELECT last_insert_rowid() AS id")
                .fetch_one(&*c)
                .await?
                .try_get("id")?
        };
        self.load_aliases(&*c, uid)
            .await?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or(Error::Code("not_found"))
    }
    pub async fn delete_alias(&self, uid: i64, id: i64) -> Result<(), Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        let n = query("DELETE FROM insight_tldr_aliases WHERE user_id=? AND id=?")
            .bind(uid)
            .bind(id)
            .execute(&*c)
            .await?;
        if n.rows_affected() == 0 {
            return Err(Error::Code("not_found"));
        }
        Ok(())
    }
    pub(crate) async fn routes<E: Executor>(
        &self,
        e: &E,
        uid: i64,
        command: &str,
    ) -> Result<Vec<Route>, Error> {
        if command != "tldr" {
            if self.config.gemini_key.is_empty() {
                return Err(Error::Code("gemini_not_configured"));
            }
            return Ok(vec![Route {
                shape: Shape::Gemini,
                base: "https://generativelanguage.googleapis.com/v1beta".into(),
                key: self.config.gemini_key.clone(),
                model: self.config.gemini_model.clone(),
                via: "gateway",
            }]);
        }
        if self.byok_enabled(e).await?
            && let Some(r)=query("SELECT * FROM insight_user_byok WHERE user_id=? AND tested_at IS NOT NULL AND test_error IS NULL").bind(uid).fetch_optional(e).await? {return Ok(vec![crate::provider::byok(&r.try_get::<String,_>("provider")?,r.try_get::<Option<String>,_>("base_url")?.as_deref(),r.try_get("api_key")?,r.try_get("model")?)?])
        }
        let allowed: Vec<String> =
            serde_json::from_str(&self.setting(e, "insight_tldr_allowed_models", "[]").await?)
                .map_err(|_| Error::Code("invalid_config"))?;
        let default = self
            .setting(e, "insight_tldr_default_model", &self.config.model)
            .await?;
        let selected = query("SELECT tldr_model FROM insight_user_settings WHERE user_id=?")
            .bind(uid)
            .fetch_optional(e)
            .await?
            .map(|r| r.try_get::<Option<String>, _>("tldr_model"))
            .transpose()?
            .flatten();
        let model = selected.filter(|m| allowed.contains(m)).unwrap_or(default);
        if model.is_empty() {
            return Err(Error::Code("no_route"));
        }
        let mode = self
            .setting(e, "insight_tldr_route_mode", &self.config.mode)
            .await?;
        let mut routes = Vec::new();
        if mode != "openrouter" {
            let base = self
                .setting(e, "insight_tldr_gateway_url", &self.config.gateway_base)
                .await?;
            if !base.is_empty() {
                routes.push(Route {
                    shape: Shape::OpenAi,
                    base: base.trim_end_matches('/').into(),
                    key: self
                        .setting(e, "insight_tldr_gateway_key", &self.config.gateway_key)
                        .await?,
                    model: model.clone(),
                    via: "gateway",
                })
            }
        }
        if mode != "a2a" {
            let key = self
                .setting(
                    e,
                    "insight_tldr_openrouter_key",
                    &self.config.openrouter_key,
                )
                .await?;
            if !key.is_empty() {
                routes.push(Route {
                    shape: Shape::OpenAi,
                    base: self.config.openrouter_base.clone(),
                    key,
                    model,
                    via: "gateway",
                })
            }
        }
        if routes.is_empty() {
            return Err(Error::Code("no_route"));
        }
        Ok(routes)
    }
    /// Inbox completion uses exactly the same current-DB routing and authorization.
    pub async fn complete(
        &self,
        uid: i64,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<String, Error> {
        self.complete_for_feature(uid, "tldr", prompt, cancel).await
    }
    pub async fn complete_for_feature(
        &self,
        uid: i64,
        feature: &str,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<String, Error> {
        if !matches!(feature, "tldr" | "inbox:classify" | "inbox:gh_organise") {
            return Err(invalid());
        }
        if prompt.chars().count() > self.config.max_chars {
            return Err(invalid());
        }
        let routes = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            self.routes(&*c, uid, "tldr").await?
        };
        let operation = self.completion.complete(&routes, prompt, cancel.clone());
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=tokio::time::timeout(self.config.request_timeout,operation)=>r.unwrap_or(Err(Error::Code("timeout")))}?;
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, Some(feature), false).await?;
        if self.routes(&*c, uid, "tldr").await? != routes {
            return Err(Error::Code("route_changed"));
        }
        Ok(result)
    }
    pub async fn summarize(
        &self,
        uid: i64,
        request: Request,
        cancel: CancellationToken,
    ) -> Result<Summary, Error> {
        self.summarize_inner(uid, request, None, cancel).await
    }
    /// Emits real provider deltas through an awaited sink. Cached summaries emit
    /// no deltas; render the returned cached Summary once. Final success is only
    /// returned after current-DB rechecks and cache/usage commit.
    pub async fn summarize_stream(
        &self,
        uid: i64,
        request: Request,
        sink: &dyn crate::provider::DeltaSink,
        cancel: CancellationToken,
    ) -> Result<Summary, Error> {
        self.summarize_inner(uid, request, Some(sink), cancel).await
    }
    async fn summarize_inner(
        &self,
        uid: i64,
        request: Request,
        sink: Option<&dyn crate::provider::DeltaSink>,
        cancel: CancellationToken,
    ) -> Result<Summary, Error> {
        if !matches!(request.command.as_str(), "summary" | "about" | "tldr")
            || request.focus.as_ref().is_some_and(|s| s.len() > 4000)
        {
            return Err(invalid());
        }
        let child = cancel.child_token();
        let operation = self.run(uid, &request, sink, child.clone());
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=tokio::time::timeout(self.config.request_timeout,operation)=>r.unwrap_or(Err(Error::Code("timeout")))};
        child.cancel();
        if let Err(error) = &result {
            let code = match error {
                Error::Code(code) => *code,
                _ => "database_error",
            };
            if let Ok(c) = self.db.connection().await
                && self.authorize(&*c, uid, None, false).await.is_ok()
            {
                let lang = query("SELECT lang FROM insight_user_settings WHERE user_id=?")
                    .bind(uid)
                    .fetch_optional(&*c)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|r| r.try_get::<String, _>(0).ok())
                    .unwrap_or("en".into());
                let video = normalize(&request.url).ok().and_then(|u| video_id(&u));
                let route = self
                    .routes(&*c, uid, &request.command)
                    .await
                    .ok()
                    .and_then(|r| r.first().map(|r| r.via))
                    .unwrap_or("gateway");
                let _ = self
                    .log(
                        &*c,
                        uid,
                        &request,
                        &lang,
                        "error",
                        Some(code),
                        video.as_deref(),
                        None,
                        None,
                        request.alias.as_deref(),
                        route,
                    )
                    .await;
            }
        }
        result
    }
    async fn run(
        &self,
        uid: i64,
        r: &Request,
        sink: Option<&dyn crate::provider::DeltaSink>,
        cancel: CancellationToken,
    ) -> Result<Summary, Error> {
        let url = normalize(&r.url)?;
        let video = video_id(&url);
        if r.command != "tldr" && video.is_none() {
            return Err(Error::Code("invalid_youtube_url"));
        }
        let feature = if r.command == "tldr" {
            "tldr"
        } else {
            "summary"
        };
        let (lang, custom, alias, search, token) = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            let row = query("SELECT * FROM insight_user_settings WHERE user_id=?")
                .bind(uid)
                .fetch_optional(&*c)
                .await?;
            let lang = row
                .as_ref()
                .map(|r| r.try_get::<String, _>("lang"))
                .transpose()?
                .unwrap_or("en".into());
            let custom =
                query("SELECT prompt FROM insight_user_prompts WHERE user_id=? AND command=?")
                    .bind(uid)
                    .bind(&r.command)
                    .fetch_optional(&*c)
                    .await?
                    .map(|r| r.try_get::<String, _>("prompt"))
                    .transpose()?;
            let alias = if r.command == "tldr" {
                prompts::resolve(
                    &url,
                    r.alias.as_deref(),
                    &self.load_aliases(&*c, uid).await?,
                )
            } else {
                None
            };
            if r.alias.is_some() && alias.is_none() {
                return Err(Error::Code("unknown_alias"));
            }
            let search = row
                .as_ref()
                .map(|r| r.try_get::<bool, _>("use_search"))
                .transpose()?
                .unwrap_or(false);
            if search {
                self.authorize(&*c, uid, Some("search"), false).await?
            }
            let token = row
                .as_ref()
                .map(|r| r.try_get::<Option<String>, _>("github_token"))
                .transpose()?
                .flatten();
            (lang, custom, alias, search, token)
        };
        // Preserve imported default Gemini cache semantics, without cross-user prompt leakage.
        if r.command != "tldr" && custom.is_none() {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            if let Some(row)=query("SELECT result FROM insight_summary_cache WHERE content_key=? AND lang=? AND command=? AND expires_at>?").bind(video.clone()).bind(&lang).bind(&r.command).bind(Utc::now()).fetch_optional(&*c).await?{let text=row.try_get("result")?;self.log(&*c,uid,r,&lang,"cached",None,video.as_deref(),None,None,None,"gateway").await?;return Ok(Summary{result:text,cached:true,route:"gateway".into(),alias:None})}
        }
        let routes = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            self.routes(&*c, uid, &r.command).await?
        };
        let content = self
            .content
            .as_ref()
            .ok_or(Error::Code("content_unavailable"))?
            .fetch(
                url.as_str(),
                crate::content::FetchOptions {
                    transcript_only: r.command != "tldr",
                    transcript_languages: &self.config.transcript_languages,
                    max_chars: self.config.max_chars,
                    github_token: if matches!(
                        url.host_str(),
                        Some(
                            "github.com"
                                | "api.github.com"
                                | "gist.github.com"
                                | "raw.githubusercontent.com"
                        )
                    ) {
                        token.as_deref()
                    } else {
                        None
                    },
                },
                cancel.clone(),
            )
            .await?;
        if content.text.trim().is_empty() {
            return Err(Error::Code("empty_content"));
        }
        let text: String = content.text.chars().take(self.config.max_chars).collect();
        let research_route = if search && self.research.is_some() {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some("search"), false).await?;
            let mut route = self.gateway_route(&*c).await?;
            if !self.config.search_model.is_empty() {
                route.model = self.config.search_model.clone()
            }
            Some(route)
        } else {
            None
        };
        let research = if search {
            let query = format!(
                "{}\n{}\n{}",
                url,
                r.focus
                    .as_deref()
                    .unwrap_or("Assess material factual claims"),
                text.chars().take(1200).collect::<String>()
            );
            let items = if let (Some(research), Some(route)) = (&self.research, &research_route) {
                research.search(route, &query, cancel.clone()).await?
            } else {
                self.content
                    .as_ref()
                    .ok_or(Error::Code("content_unavailable"))?
                    .research(&query, cancel.clone())
                    .await?
            };
            items
                .into_iter()
                .filter(|c| normalize(&c.url).is_ok_and(|u| u != url))
                .take(10)
                .map(|mut c| {
                    c.text = c.text.chars().take(5000).collect();
                    c
                })
                .collect()
        } else {
            Vec::new()
        };
        let prompt = prompts::build(
            &r.command,
            &lang,
            &text,
            custom.as_deref(),
            alias.as_ref().map(|a| a.1.as_str()),
            r.focus.as_deref(),
            &research,
        );
        let key = format!(
            "{:x}",
            Sha256::digest(format!(
                "{uid}\n{}\n{}\n{routes:?}\n{}\n{prompt}",
                url,
                r.command,
                routes
                    .iter()
                    .map(|r| format!("{}:{}", r.base, r.key))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        );
        let via = routes[0].via;
        let alias_key = alias.as_ref().map(|a| a.0.as_str());
        let reservation = format!(
            "{:x}",
            Sha256::digest(format!(
                "{key}:{}",
                Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ))
        );
        {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            if let Some(row)=query("SELECT result FROM insight_native_cache WHERE cache_key=? AND user_id=? AND expires_at>?").bind(&key).bind(uid).bind(Utc::now()).fetch_optional(&*c).await?{self.log(&*c,uid,r,&lang,"cached",None,video.as_deref(),Some(text.chars().count() as i32),content.video_seconds,alias_key,via).await?;return Ok(Summary{result:row.try_get("result")?,cached:true,route:via.into(),alias:alias.map(|a|a.0)})}
            let now = Utc::now();
            query("DELETE FROM insight_native_reservations WHERE expires_at<=?")
                .bind(now)
                .execute(&*c)
                .await?;
            query("DELETE FROM insight_native_cache WHERE expires_at<=?")
                .bind(now)
                .execute(&*c)
                .await?;
            let day = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
            let limit = if feature == "summary" {
                self.config.summary_limit
            } else {
                self.config.tldr_limit
            };
            let n:i64=query("SELECT (SELECT count(*) FROM insight_usage_log WHERE user_id=? AND status='ok' AND created_at>=? AND ((?='tldr' AND command LIKE 'tldr%') OR (?='summary' AND command IN ('summary','about')))) + (SELECT count(*) FROM insight_native_reservations WHERE user_id=? AND command=? AND created_at>=? AND expires_at>?) AS n").bind(uid).bind(day).bind(feature).bind(feature).bind(uid).bind(feature).bind(day).bind(now).fetch_one(&*c).await?.try_get("n")?;
            if limit > 0 && n >= limit {
                return Err(Error::Code("quota_exceeded"));
            }
            query("INSERT INTO insight_native_reservations(id,user_id,command,created_at,expires_at) VALUES(?,?,?,?,?)").bind(&reservation).bind(uid).bind(feature).bind(now).bind(now+chrono::Duration::seconds(self.config.request_timeout.as_secs() as i64+5)).execute(&*c).await?;
        }
        let outcome = if let Some(sink) = sink {
            let guarded = AuthorizedSink {
                service: self,
                sink,
                uid,
                feature,
                routes: &routes,
                command: &r.command,
                search,
                research_route: research_route.as_ref(),
            };
            self.completion
                .stream(&routes, &prompt, &guarded, cancel)
                .await
        } else {
            self.completion.complete(&routes, &prompt, cancel).await
        };
        let mut c = self.db.connection().await?;
        query("DELETE FROM insight_native_reservations WHERE id=?")
            .bind(&reservation)
            .execute(&*c)
            .await?;
        self.authorize(&*c, uid, Some(feature), false).await?;
        if search {
            self.authorize(&*c, uid, Some("search"), false).await?
        }
        if let Some(route) = &research_route {
            let mut current = self.gateway_route(&*c).await?;
            current.model = route.model.clone();
            if current != *route {
                return Err(Error::Code("route_changed"));
            }
        }
        // Credentials/routes may have been revoked or replaced during provider I/O.
        if self.routes(&*c, uid, &r.command).await? != routes {
            return Err(Error::Code("route_changed"));
        }
        match outcome {
            Ok(result) => {
                let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
                    .await
                    .map_err(|_| Error::Database)?;
                let saved=async {
                query("INSERT INTO insight_native_cache(cache_key,user_id,result,expires_at) VALUES(?,?,?,?) ON CONFLICT(cache_key) DO UPDATE SET result=excluded.result,expires_at=excluded.expires_at").bind(&key).bind(uid).bind(&result).bind(Utc::now()+chrono::Duration::hours(24)).execute(&*tx).await?;
                self.log(
                    &*tx,
                    uid,
                    r,
                    &lang,
                    "ok",
                    None,
                    video.as_deref(),
                    Some(text.chars().count() as i32),
                    content.video_seconds,
                    alias_key,
                    via,
                )
                .await?;
                Ok::<_,Error>(())
                }.await;
                match saved {
                    Ok(()) => tx.commit().await.map_err(|_| Error::Database)?,
                    Err(error) => {
                        let _ = tx.rollback().await;
                        return Err(error);
                    }
                }
                Ok(Summary {
                    result,
                    cached: false,
                    route: via.into(),
                    alias: alias.map(|a| a.0),
                })
            }
            Err(error) => Err(error),
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn log<E: Executor>(
        &self,
        e: &E,
        uid: i64,
        r: &Request,
        lang: &str,
        status: &str,
        error: Option<&str>,
        video: Option<&str>,
        chars: Option<i32>,
        seconds: Option<i32>,
        alias: Option<&str>,
        via: &str,
    ) -> Result<(), Error> {
        query("INSERT INTO insight_usage_log(user_id,command,video_id,lang,status,error_code,created_at,content_chars,video_seconds,alias_key,route) VALUES(?,?,?,?,?,?,?,?,?,?,?)").bind(uid).bind(&r.command).bind(video).bind(lang).bind(status).bind(error).bind(Utc::now()).bind(chars).bind(seconds).bind(alias).bind(via).execute(e).await?;
        Ok(())
    }
    pub async fn settings(&self, uid: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        self.settings_on(&*c, uid).await
    }
    pub(crate) async fn settings_on<E: Executor>(&self, e: &E, uid: i64) -> Result<Value, Error> {
        let row = query("SELECT * FROM insight_user_settings WHERE user_id=?")
            .bind(uid)
            .fetch_optional(e)
            .await?;
        let mut prompts = serde_json::Map::new();
        for r in query("SELECT command,prompt FROM insight_user_prompts WHERE user_id=?")
            .bind(uid)
            .fetch_all(e)
            .await?
        {
            prompts.insert(
                r.try_get("command")?,
                json!(r.try_get::<String, _>("prompt")?),
            );
        }
        let summary = self.authorize(e, uid, Some("summary"), false).await.is_ok();
        let tldr = self.authorize(e, uid, Some("tldr"), false).await.is_ok();
        let search = self.authorize(e, uid, Some("search"), false).await.is_ok();
        let gateway = self.authorize_gateway(e, uid, "tldr").await.is_ok();
        Ok(
            json!({"lang":row.as_ref().map(|r|r.try_get::<String,_>("lang")).transpose()?.unwrap_or("en".into()),"has_gemini_access":summary,"has_tldr_access":tldr,"has_tldr_gateway_access":gateway,"has_search_access":search,"has_search_gateway_access":search,"tldr_model":row.as_ref().map(|r|r.try_get::<Option<String>,_>("tldr_model")).transpose()?.flatten(),"tldr_allowed_models":serde_json::from_str::<Value>(&self.setting(e,"insight_tldr_allowed_models","[]").await?).unwrap_or(json!([])),"github_token_set":row.as_ref().map(|r|r.try_get::<Option<String>,_>("github_token")).transpose()?.flatten().is_some(),"use_search":row.as_ref().map(|r|r.try_get::<bool,_>("use_search")).transpose()?.unwrap_or(false),"prompts":prompts,"prompt_defaults":{"summary":prompts::SUMMARY,"about":prompts::ABOUT,"tldr":prompts::TLDR},"alias_defaults":{"max":prompts::builtin("max"),"nobullshit":prompts::builtin("nobullshit"),"noshit":prompts::builtin("noshit"),"tale":prompts::builtin("tale")}}),
        )
    }
}

struct AuthorizedSink<'a> {
    service: &'a Insight,
    sink: &'a dyn crate::provider::DeltaSink,
    uid: i64,
    feature: &'a str,
    routes: &'a [Route],
    command: &'a str,
    search: bool,
    research_route: Option<&'a Route>,
}
impl crate::provider::DeltaSink for AuthorizedSink<'_> {
    fn delta<'a>(&'a self, text: &'a str) -> crate::FutureResult<'a, ()> {
        Box::pin(async move {
            {
                let c = self.service.db.connection().await?;
                self.service
                    .authorize(&*c, self.uid, Some(self.feature), false)
                    .await?;
                if self.search {
                    self.service
                        .authorize(&*c, self.uid, Some("search"), false)
                        .await?
                }
                if self.service.routes(&*c, self.uid, self.command).await? != self.routes {
                    return Err(Error::Code("route_changed"));
                }
                if let Some(route) = self.research_route {
                    let mut current = self.service.gateway_route(&*c).await?;
                    current.model = route.model.clone();
                    if current != *route {
                        return Err(Error::Code("route_changed"));
                    }
                }
            }
            self.sink.delta(text).await
        })
    }
}
