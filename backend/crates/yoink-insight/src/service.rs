use crate::{
    Error,
    content::{ContentSource, normalize, video_id},
    invalid,
    prompts::{self, Alias, AliasInput},
    provider::{Completion, DeltaSink, Route, Shape},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::{
    sync::CancellationToken,
    task::{TaskTracker, task_tracker::TaskTrackerToken},
};
use turso::transaction::TransactionBehavior;
use yoink_access::{FeatureSpec, Grant, Principal, Role, effective_grant};
use yoink_db::Transaction;
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
    pub default_lang: String,
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
            default_lang: "en".into(),
            request_timeout: Duration::from_secs(120),
        }
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InsightConfig([REDACTED])")
    }
}
/// Committed authorization changes: Some(user) targets one user; None invalidates all.
/// The coordinator binds this transport-neutral callback to its identity notifier.
pub type AccessChangeCallback = Arc<dyn Fn(Option<i64>) + Send + Sync>;
#[derive(Clone)]
pub struct Insight {
    pub db: Database,
    pub config: Config,
    pub content: Option<Arc<dyn ContentSource>>,
    pub completion: Arc<dyn Completion>,
    pub research: Option<Arc<dyn crate::research::Research>>,
    access_change_callback: Option<AccessChangeCallback>,
    quota_lifecycle: Arc<QuotaLifecycle>,
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
            || !crate::management::valid_language(&config.default_lang)
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
            access_change_callback: None,
            quota_lifecycle: Arc::new(QuotaLifecycle::default()),
        })
    }
    /// Permanently close quota admission across all clones and wait for live
    /// reservation owners and their drop cleanup. Idempotent and cancellation-safe:
    /// cancelling this wait does not reopen admission or cancel cleanup. The
    /// coordinator must drain adapters and cancel/join content and OAuth work
    /// before awaiting this fence, and keep the DB and Tokio runtime open until it
    /// returns. DB cleanup failures retain the reservation expiry backstop.
    pub async fn shutdown_quota_cleanup(&self) {
        {
            let mut closing = self.quota_lifecycle.closing.lock().unwrap();
            *closing = true;
            self.quota_lifecycle.tracker.close();
        }
        self.quota_lifecycle.tracker.wait().await;
    }
    /// Install a notifier before cloning the service into adapters. Builders preserve it.
    pub fn with_access_change_callback(mut self, callback: AccessChangeCallback) -> Self {
        self.access_change_callback = Some(callback);
        self
    }
    pub(crate) fn notify_access_change(&self, user_id: Option<i64>) {
        if let Some(callback) = &self.access_change_callback {
            callback(user_id);
        }
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
                let byok=provider_grants&&plugin=="insight"&&feature=="tldr"&&self.byok_enabled(e).await?&&query("SELECT 1 FROM insight_user_byok WHERE user_id=? AND tested_at IS NOT NULL AND test_error IS NULL AND api_key<>''").bind(uid).fetch_optional(e).await?.is_some();
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
        self.authorize(&*c, uid, Some("tldr"), false).await?;
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
        mut a: AliasInput,
    ) -> Result<Alias, Error> {
        let c = self.db.connection().await?;
        self.authorize(
            &*c,
            uid,
            if id.is_none() { Some("tldr") } else { None },
            false,
        )
        .await?;
        let rows = self.load_aliases(&*c, uid).await?;
        if id.is_some_and(|id| !rows.iter().any(|r| r.id == id)) {
            return Err(Error::Code("not_found"));
        }
        if let Some(old) = rows.iter().find(|r| Some(r.id) == id) {
            a.aliases = a.aliases.or_else(|| old.aliases.clone());
            a.prompt = a.prompt.or_else(|| old.prompt.clone());
            a.domains = a.domains.or_else(|| old.domains.clone());
            a.target_alias = a.target_alias.or_else(|| old.target_alias.clone());
        }
        a.aliases = a
            .aliases
            .map(|value| prompts::csv(Some(&value)).join(", "))
            .filter(|value| !value.is_empty());
        a.domains = a
            .domains
            .map(|value| prompts::csv(Some(&value)).join(", "))
            .filter(|value| !value.is_empty());
        a.prompt = a
            .prompt
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        a.target_alias = a
            .target_alias
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty());
        a.validate()?;
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
            && !self.legacy_gateway_precedence(e, uid).await?
            && let Some(r)=query("SELECT * FROM insight_user_byok WHERE user_id=? AND tested_at IS NOT NULL AND test_error IS NULL AND api_key<>''").bind(uid).fetch_optional(e).await? {return Ok(vec![crate::provider::byok(&r.try_get::<String,_>("provider")?,r.try_get::<Option<String>,_>("base_url")?.as_deref(),r.try_get("api_key")?,r.try_get("model")?)?])
        }
        let allowed: Vec<String> = serde_json::from_str(&self.allowed_models(e).await?)
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
        let owner = self.is_owner(e, uid).await?;
        let model = selected
            .filter(|m| owner || allowed.contains(m))
            .unwrap_or(default);
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
                    via: "openrouter",
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
        let reservation_id = format!(
            "{:x}",
            Sha256::digest(format!(
                "{uid}:{}",
                Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ))
        );
        let mut cleanup = QuotaReservation {
            db: self.db.clone(),
            id: reservation_id.clone(),
            token: Some(self.quota_lifecycle.admit()?),
            armed: false,
        };
        let operation = self.run(uid, &request, sink, child.clone(), &mut cleanup);
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Code("cancelled")),r=tokio::time::timeout(self.config.request_timeout,operation)=>r.unwrap_or(Err(Error::Code("timeout")))};
        child.cancel();
        cleanup.release().await;
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
                    .unwrap_or(self.config.default_lang.clone());
                let video = normalize(&request.url).ok().and_then(|u| video_id(&u));
                let route = self
                    .routes(&*c, uid, &request.command)
                    .await
                    .ok()
                    .and_then(|r| {
                        r.first()
                            .map(|r| if r.is_byok() { "byok" } else { "gateway" })
                    })
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
        cleanup: &mut QuotaReservation,
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
                .unwrap_or(self.config.default_lang.clone());
            let custom =
                query("SELECT prompt FROM insight_user_prompts WHERE user_id=? AND command=?")
                    .bind(uid)
                    .bind(&r.command)
                    .fetch_optional(&*c)
                    .await?
                    .map(|r| r.try_get::<String, _>("prompt"))
                    .transpose()?;
            let alias = if r.command == "tldr" && (r.alias.is_some() || r.focus.is_none()) {
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
            let search = r.command == "tldr"
                && self
                    .routes(&*c, uid, &r.command)
                    .await?
                    .first()
                    .is_some_and(|route| {
                        route.via == "gateway" && route.base != self.config.openrouter_base
                    })
                && self.authorize_gateway(&*c, uid, "search").await.is_ok()
                && row
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
        // Legacy shared cache rows lack producer/user/prompt/credential provenance.
        // Preserve them for migration/audit, but never serve their unverifiable results.
        // Only the user- and configuration-bound native cache below is eligible.
        let routes = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            self.routes(&*c, uid, &r.command).await?
        };
        let mut reserved = false;
        if feature == "tldr" {
            self.reserve_quota(uid, feature, cleanup).await?;
            reserved = true;
        }
        let instruction = prompts::build(
            &r.command,
            &lang,
            "",
            custom.as_deref(),
            alias.as_ref().map(|entry| entry.1.as_str()),
            None,
            &[],
        );
        let key = format!(
            "{:x}",
            Sha256::digest(format!(
                "replay-v1:{uid}\n{}\n{}\n{routes:?}\n{}\n{lang}\n{custom:?}\n{alias:?}\n{token:?}\n{search}\n{instruction}\n{}",
                url,
                r.command,
                routes
                    .iter()
                    .map(|r| format!("{}:{}", r.base, r.key))
                    .collect::<Vec<_>>()
                    .join("\n"),
                self.config.search_model
            ))
        );
        let via = if routes[0].is_byok() {
            "byok"
        } else {
            "gateway"
        };
        let alias_key = alias.as_ref().map(|a| a.0.as_str());
        let mut guarded = sink.map(|sink| AuthorizedSink {
            service: self,
            sink,
            uid,
            feature,
            routes: &routes,
            command: &r.command,
            search,
            research_route: None,
        });
        if r.focus.is_none() {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, Some(feature), false).await?;
            if let Some(row) = query("SELECT result FROM insight_native_cache WHERE cache_key=? AND user_id=? AND expires_at>?").bind(&key).bind(uid).bind(Utc::now()).fetch_optional(&*c).await? {
                self.log(&*c, uid, r, &lang, "cached", None, video.as_deref(), None, None, alias_key, via).await?;
                let result = row.try_get("result")?;
                drop(c);
                if let Some(sink) = &guarded { sink.phase(crate::streaming::Phase::Cached, url.as_str(), alias_key).await?; }
                return Ok(Summary { result, cached: true, route: via.into(), alias: alias.map(|a| a.0) });
            }
        }
        if let Some(sink) = &guarded {
            sink.phase(crate::streaming::Phase::Fetching, url.as_str(), alias_key)
                .await?;
        }
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
        if let Some(sink) = &mut guarded {
            sink.research_route = research_route.as_ref();
        }
        let research = if search {
            if let Some(sink) = &guarded {
                sink.phase(
                    crate::streaming::Phase::Researching,
                    url.as_str(),
                    alias_key,
                )
                .await?;
            }
            let query = format!(
                "{}\n{}\n{}",
                url,
                r.focus
                    .as_deref()
                    .unwrap_or("Assess material factual claims"),
                text.chars().take(1200).collect::<String>()
            );
            let items = if let (Some(research), Some(route)) = (&self.research, &research_route) {
                match research.search(route, &query, cancel.clone()).await {
                    Ok(items) => items,
                    Err(Error::Code("cancelled")) => return Err(Error::Code("cancelled")),
                    Err(_) => Vec::new(),
                }
            } else {
                match self
                    .content
                    .as_ref()
                    .ok_or(Error::Code("content_unavailable"))?
                    .research(&query, cancel.clone())
                    .await
                {
                    Ok(items) => items,
                    Err(Error::Code("cancelled")) => return Err(Error::Code("cancelled")),
                    Err(_) => Vec::new(),
                }
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
        if !reserved {
            self.reserve_quota(uid, feature, cleanup).await?;
        }
        if let Some(sink) = &guarded {
            sink.phase(crate::streaming::Phase::Generating, url.as_str(), alias_key)
                .await?;
        }
        let outcome = if let Some(guarded) = &guarded {
            self.completion
                .stream(&routes, &prompt, guarded, cancel)
                .await
        } else {
            self.completion.complete(&routes, &prompt, cancel).await
        };
        let mut c = self.db.connection().await?;

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
                if r.focus.is_none() { query("INSERT INTO insight_native_cache(cache_key,user_id,result,expires_at) VALUES(?,?,?,?) ON CONFLICT(cache_key) DO UPDATE SET result=excluded.result,expires_at=excluded.expires_at").bind(&key).bind(uid).bind(&result).bind(Utc::now()+chrono::Duration::hours(24)).execute(&*tx).await?; }
                query("DELETE FROM insight_native_reservations WHERE id=?").bind(&cleanup.id).execute(&*tx).await?;
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
                cleanup.armed = false;
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
        let search_gateway = self.authorize_gateway(e, uid, "search").await.is_ok();
        let gateway = self.authorize_gateway(e, uid, "tldr").await.is_ok();
        Ok(
            json!({"lang":row.as_ref().map(|r|r.try_get::<String,_>("lang")).transpose()?.unwrap_or(self.config.default_lang.clone()),"has_gemini_access":summary,"has_tldr_access":tldr,"has_tldr_gateway_access":gateway,"has_search_access":search,"has_search_gateway_access":search_gateway,"tldr_model":if gateway { row.as_ref().map(|r|r.try_get::<Option<String>,_>("tldr_model")).transpose()?.flatten() } else { None },"tldr_allowed_models":if gateway { serde_json::from_str::<Value>(&self.allowed_models(e).await?).unwrap_or(json!([])) } else { json!([]) },"github_token_set":tldr && row.as_ref().map(|r|r.try_get::<Option<String>,_>("github_token")).transpose()?.flatten().is_some(),"use_search":search_gateway && row.as_ref().map(|r|r.try_get::<bool,_>("use_search")).transpose()?.unwrap_or(false),"prompts":prompts,"prompt_defaults":{"summary":prompts::SUMMARY,"about":prompts::ABOUT,"tldr":prompts::TLDR},"alias_defaults":{"max":prompts::builtin("max"),"nobullshit":prompts::builtin("nobullshit"),"noshit":prompts::builtin("noshit"),"tale":prompts::builtin("tale")}}),
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
impl AuthorizedSink<'_> {
    async fn require(&self) -> Result<(), Error> {
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
        Ok(())
    }
}
impl crate::provider::DeltaSink for AuthorizedSink<'_> {
    fn delta<'a>(&'a self, text: &'a str) -> crate::FutureResult<'a, ()> {
        Box::pin(async move {
            self.require().await?;
            self.sink.delta(text).await
        })
    }
    fn phase<'a>(
        &'a self,
        phase: crate::streaming::Phase,
        source: &'a str,
        alias: Option<&'a str>,
    ) -> crate::FutureResult<'a, ()> {
        Box::pin(async move {
            self.require().await?;
            self.sink.phase(phase, source, alias).await?;
            // Status delivery can suspend before the next external operation.
            self.require().await
        })
    }
}

/// Shared admission gate and owner/cleanup lifetime tracking across clones.
#[derive(Default)]
struct QuotaLifecycle {
    closing: Mutex<bool>,
    tracker: TaskTracker,
}
impl QuotaLifecycle {
    fn admit(&self) -> Result<TaskTrackerToken, Error> {
        let closing = self.closing.lock().unwrap();
        if *closing {
            return Err(Error::Code("shutting_down"));
        }
        // close() alone permits late tokens/spawns. Serialize registration with
        // closing, then retain this token until the owner/cleanup is finished.
        Ok(self.tracker.token())
    }
}
/// A dropped/cancelled future releases its admission lease without permitting a
/// partial response to commit. The expiry is a crash/restart backstop only.
struct QuotaReservation {
    db: Database,
    id: String,
    token: Option<TaskTrackerToken>,
    armed: bool,
}
impl Drop for QuotaReservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let db = self.db.clone();
        let id = self.id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let token = self.token.take();
            runtime.spawn(async move {
                // Move, rather than replace, the live token: no empty-tracker gap
                // is possible even when shutdown has already closed the tracker.
                let _token = token;
                if let Ok(connection) = db.connection().await {
                    let _ = query("DELETE FROM insight_native_reservations WHERE id=?")
                        .bind(id)
                        .execute(&*connection)
                        .await;
                }
            });
        }
    }
}
impl Insight {
    async fn reserve_quota(
        &self,
        uid: i64,
        feature: &str,
        cleanup: &mut QuotaReservation,
    ) -> Result<(), Error> {
        if *self.quota_lifecycle.closing.lock().unwrap() {
            return Err(Error::Code("shutting_down"));
        }
        let mut connection = self.db.connection().await?;
        self.authorize(&*connection, uid, Some(feature), false)
            .await?;
        let transaction = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        let now = Utc::now();
        query("DELETE FROM insight_native_reservations WHERE expires_at<=?")
            .bind(now)
            .execute(&*transaction)
            .await?;
        query("DELETE FROM insight_native_cache WHERE expires_at<=?")
            .bind(now)
            .execute(&*transaction)
            .await?;
        let day = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
        let limit = if feature == "summary" {
            self.config.summary_limit
        } else {
            self.config.tldr_limit
        };
        let count: i64 = query("SELECT (SELECT count(*) FROM insight_usage_log WHERE user_id=? AND status='ok' AND created_at>=? AND (?='summary' OR command='tldr')) + (SELECT count(*) FROM insight_native_reservations WHERE user_id=? AND (?='summary' OR command='tldr') AND created_at>=? AND expires_at>?) AS n").bind(uid).bind(day).bind(feature).bind(uid).bind(feature).bind(day).bind(now).fetch_one(&*transaction).await?.try_get("n")?;
        if limit > 0 && count >= limit {
            transaction.rollback().await.map_err(|_| Error::Database)?;
            return Err(Error::Code("quota_exceeded"));
        }
        let id = &cleanup.id;
        // Arm before insertion/commit: cancellation may leave commit outcome
        // uncertain. Drop cleanup is serialized behind the held DB connection.
        cleanup.armed = true;
        query("INSERT INTO insight_native_reservations(id,user_id,command,created_at,expires_at) VALUES(?,?,?,?,?)").bind(id).bind(uid).bind(feature).bind(now).bind(now + chrono::Duration::seconds(self.config.request_timeout.as_secs() as i64 + 5)).execute(&*transaction).await?;
        transaction.commit().await.map_err(|_| Error::Database)?;
        Ok(())
    }
}

impl QuotaReservation {
    async fn release(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(connection) = self.db.connection().await
            && query("DELETE FROM insight_native_reservations WHERE id=?")
                .bind(&self.id)
                .execute(&*connection)
                .await
                .is_ok()
        {
            self.armed = false;
        }
    }
}

impl Insight {
    /// The executable legacy resolver checks InsightAccessService, whose feature
    /// is summary (not the tldr feature named in its docstring).
    async fn legacy_gateway_precedence<E: Executor>(
        &self,
        executor: &E,
        uid: i64,
    ) -> Result<bool, Error> {
        if self.is_owner(executor, uid).await? {
            return Ok(true);
        }
        Ok(query("SELECT 1 FROM user_permissions WHERE user_id=? AND plugin='insight' AND feature='summary' AND (expires_at IS NULL OR expires_at>?)").bind(uid).bind(Utc::now()).fetch_optional(executor).await?.is_some())
    }
}
