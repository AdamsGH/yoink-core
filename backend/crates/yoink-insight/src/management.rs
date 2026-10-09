use crate::{Error, Insight, invalid, provider};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_db::{Executor, query};
#[derive(Deserialize)]
pub struct SettingsUpdate {
    pub lang: String,
    #[serde(default, deserialize_with = "nullable_model")]
    pub tldr_model: Option<Option<String>>,
    pub github_token: Option<String>,
    pub use_search: Option<bool>,
    pub prompts: Option<std::collections::BTreeMap<String, Option<String>>>,
}
#[derive(Deserialize)]
pub struct ByokUpdate {
    pub provider: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: String,
}
impl Insight {
    pub async fn update_settings(&self, uid: i64, mut u: SettingsUpdate) -> Result<Value, Error> {
        if !valid_language(&u.lang) || u.github_token.as_ref().is_some_and(|s| s.len() > 256) {
            return Err(invalid());
        }
        if let Some(p) = &u.prompts
            && p.iter().any(|(k, v)| {
                !matches!(k.as_str(), "summary" | "about" | "tldr")
                    || v.as_ref().is_some_and(|s| s.len() > 20000)
            })
        {
            return Err(invalid());
        }
        let mut c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        if u.github_token.is_some() {
            self.authorize(&*c, uid, Some("tldr"), false).await?;
        }
        if u.tldr_model.is_some() {
            self.authorize_gateway(&*c, uid, "tldr").await?;
        }
        if u.use_search == Some(true) {
            self.authorize_gateway(&*c, uid, "search").await?
        }
        let summary = self
            .authorize(&*c, uid, Some("summary"), false)
            .await
            .is_ok();
        let tldr = self.authorize(&*c, uid, Some("tldr"), false).await.is_ok();
        if !summary && !tldr {
            return Err(Error::Code("access_denied"));
        }
        if let Some(prompts) = &mut u.prompts {
            prompts.retain(|command, _| if command == "tldr" { tldr } else { summary });
        }
        if let Some(Some(model)) = &u.tldr_model {
            let allowed: Vec<String> =
                serde_json::from_str(&self.allowed_models(&*c).await?).map_err(|_| invalid())?;
            if !allowed.contains(model) && !self.is_owner(&*c, uid).await? {
                return Err(Error::Code("model_not_allowed"));
            }
        }
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        let result=async{query("INSERT INTO insight_user_settings(user_id,lang) VALUES(?,?) ON CONFLICT(user_id) DO UPDATE SET lang=excluded.lang").bind(uid).bind(&u.lang).execute(&*tx).await?;
 if let Some(model)=u.tldr_model{query("UPDATE insight_user_settings SET tldr_model=? WHERE user_id=?").bind(model).bind(uid).execute(&*tx).await?;}
 if let Some(token)=u.github_token{query("UPDATE insight_user_settings SET github_token=? WHERE user_id=?").bind(if token.is_empty(){None}else{Some(token)}).bind(uid).execute(&*tx).await?;}
 if let Some(search)=u.use_search{query("UPDATE insight_user_settings SET use_search=? WHERE user_id=?").bind(search).bind(uid).execute(&*tx).await?;}
 if let Some(prompts)=u.prompts{for (command,prompt) in prompts {if let Some(prompt)=prompt.filter(|s|!s.trim().is_empty()){query("INSERT INTO insight_user_prompts(user_id,command,prompt,updated_at) VALUES(?,?,?,?) ON CONFLICT(user_id,command) DO UPDATE SET prompt=excluded.prompt,updated_at=excluded.updated_at").bind(uid).bind(command).bind(prompt).bind(Utc::now()).execute(&*tx).await?;}else{query("DELETE FROM insight_user_prompts WHERE user_id=? AND command=?").bind(uid).bind(command).execute(&*tx).await?;}}}Ok::<_,Error>(())}.await;
        match result {
            Ok(()) => {
                tx.commit().await.map_err(|_| Error::Database)?;
                self.settings_on(&*c, uid).await
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    pub async fn byok_view(&self, uid: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        self.byok_on(&*c, uid).await
    }
    async fn byok_on<E: Executor>(&self, e: &E, uid: i64) -> Result<Value, Error> {
        let row = query("SELECT * FROM insight_user_byok WHERE user_id=?")
            .bind(uid)
            .fetch_optional(e)
            .await?;
        let mut out = json!({"enabled":self.byok_enabled(e).await?,"has_config":row.is_some(),"api_key_set":row.is_some(),"api_key_masked":null,"provider":null,"base_url":null,"model":null,"models":[],"models_fetched_at":null,"tested_at":null,"test_error":null,"providers":[{"id":"openai","label":"OpenAI","default_base_url":"https://api.openai.com/v1","requires_base_url":false,"api_shape":"openai","all_websearch":false},{"id":"anthropic","label":"Anthropic","default_base_url":"https://api.anthropic.com/v1","requires_base_url":false,"api_shape":"anthropic","all_websearch":false},{"id":"gemini","label":"Gemini","default_base_url":"https://generativelanguage.googleapis.com/v1beta/openai","requires_base_url":false,"api_shape":"openai","all_websearch":false},{"id":"openrouter","label":"OpenRouter","default_base_url":"https://openrouter.ai/api/v1","requires_base_url":false,"api_shape":"openai","all_websearch":false},{"id":"perplexity","label":"Perplexity","default_base_url":"https://api.perplexity.ai","requires_base_url":false,"api_shape":"openai","all_websearch":true},{"id":"custom_openai","label":"Custom OpenAI-compatible","requires_base_url":true,"api_shape":"openai","all_websearch":false},{"id":"custom_anthropic","label":"Custom Anthropic-compatible","requires_base_url":true,"api_shape":"anthropic","all_websearch":false}]});
        if let Some(r) = row {
            let key: String = r.try_get("api_key")?;
            out["api_key_set"] = json!(!key.is_empty());
            let chars = key.chars().collect::<Vec<_>>();
            out["api_key_masked"] = if chars.len() > 4 {
                json!(format!(
                    "...{}",
                    chars[chars.len() - 4..].iter().collect::<String>()
                ))
            } else if !chars.is_empty() {
                json!("***")
            } else {
                Value::Null
            };
            for name in ["provider", "base_url", "model"] {
                out[name] = json!(r.try_get::<Option<String>, _>(name)?)
            }
            for name in ["models_fetched_at", "tested_at"] {
                out[name] = json!(r.try_get::<Option<chrono::DateTime<Utc>>, _>(name)?)
            }
            out["test_error"] = json!(
                r.try_get::<Option<String>, _>("test_error")?
                    .map(|_| "provider_probe_failed")
            );
            out["models"] = r
                .try_get::<Option<String>, _>("models_json")?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(json!([]));
        }
        Ok(out)
    }
    pub async fn save_byok(&self, uid: i64, u: ByokUpdate) -> Result<Value, Error> {
        self.save_byok_with_cancel(uid, u, tokio_util::sync::CancellationToken::new())
            .await
    }
    pub async fn save_byok_with_cancel(
        &self,
        uid: i64,
        mut u: ByokUpdate,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Code("cancelled"));
        }
        u.model = u.model.trim().into();
        u.base_url = u
            .base_url
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        u.api_key = u.api_key.map(|s| s.trim().to_owned());
        if u.api_key.as_deref() == Some("") {
            let c = self.db.connection().await?;
            self.authorize_byok_management(&*c, uid).await?;
            let changed = query("UPDATE insight_user_byok SET api_key='',tested_at=NULL,test_error='api_key_required',updated_at=? WHERE user_id=?").bind(Utc::now()).bind(uid).execute(&*c).await?;
            if changed.rows_affected() > 0 {
                self.notify_access_change(Some(uid));
            }
            let view = self.byok_on(&*c, uid).await;
            drop(c);
            return view;
        }
        let (route, snapshot) = {
            let c = self.db.connection().await?;
            self.authorize_byok_management(&*c, uid).await?;
            let snapshot = self.byok_revision(&*c, uid).await?;
            let key = u
                .api_key
                .clone()
                .or_else(|| snapshot.as_ref().map(|s| s.2.clone()))
                .ok_or(Error::Code("api_key_required"))?;
            (
                provider::byok(&u.provider, u.base_url.as_deref(), key, u.model.clone())?,
                snapshot,
            )
        };
        // As in the pinned PUT, probe before persistence. No DB lock crosses I/O.
        let result = self.catalogue_call(&route, cancel).await;
        let c = self.db.connection().await?;
        self.authorize_byok_management(&*c, uid).await?;
        if self.byok_revision(&*c, uid).await? != snapshot {
            return Err(Error::Code("credential_changed"));
        }
        if matches!(result, Err(Error::Code("cancelled"))) {
            return Err(Error::Code("cancelled"));
        }
        let ok = result.is_ok();
        let models = result
            .ok()
            .map(|m| serde_json::to_string(&m))
            .transpose()
            .map_err(|_| invalid())?;
        let now = Utc::now();
        let error = if ok {
            None
        } else {
            Some("provider_probe_failed")
        };
        // Failed replacement keeps the frontend's previous catalogue/timestamps,
        // but test_error prevents it becoming a valid credential/provider grant.
        query("INSERT INTO insight_user_byok(user_id,provider,base_url,api_key,model,models_json,models_fetched_at,tested_at,test_error,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET provider=excluded.provider,base_url=excluded.base_url,api_key=excluded.api_key,model=excluded.model,models_json=CASE WHEN ? THEN excluded.models_json ELSE insight_user_byok.models_json END,models_fetched_at=CASE WHEN ? THEN excluded.models_fetched_at ELSE insight_user_byok.models_fetched_at END,tested_at=CASE WHEN ? THEN excluded.tested_at ELSE insight_user_byok.tested_at END,test_error=excluded.test_error,updated_at=excluded.updated_at")
            .bind(uid).bind(u.provider).bind(u.base_url).bind(&route.key).bind(u.model).bind(models).bind(if ok {Some(now)} else {None}).bind(if ok {Some(now)} else {None}).bind(error).bind(now).bind(now).bind(ok).bind(ok).bind(ok).execute(&*c).await?;
        self.notify_access_change(Some(uid));
        let view = self.byok_on(&*c, uid).await;
        drop(c);
        view
    }
    async fn byok_revision<E: Executor>(
        &self,
        e: &E,
        uid: i64,
    ) -> Result<Option<ByokRevision>, Error> {
        let row = query("SELECT provider,base_url,api_key,model,updated_at FROM insight_user_byok WHERE user_id=?").bind(uid).fetch_optional(e).await?;
        row.map(|r| {
            Ok((
                r.try_get("provider")?,
                r.try_get("base_url")?,
                r.try_get("api_key")?,
                r.try_get("model")?,
                r.try_get("updated_at")?,
            ))
        })
        .transpose()
    }
    pub async fn delete_byok(&self, uid: i64) -> Result<(), Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        let deleted = query("DELETE FROM insight_user_byok WHERE user_id=?")
            .bind(uid)
            .execute(&*c)
            .await?;
        drop(c);
        if deleted.rows_affected() > 0 {
            self.notify_access_change(Some(uid));
        }
        Ok(())
    }
    /// Connectivity probe uses the bounded completion adapter; persisted result is
    /// conditional on the exact credential revision loaded before async I/O.
    pub async fn probe_byok(
        &self,
        uid: i64,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        let (route, revision) = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, None, false).await?;
            self.authorize_byok_management(&*c, uid).await?;
            let row = query("SELECT * FROM insight_user_byok WHERE user_id=?")
                .bind(uid)
                .fetch_optional(&*c)
                .await?
                .ok_or(Error::Code("not_found"))?;
            (
                provider::byok(
                    &row.try_get::<String, _>("provider")?,
                    row.try_get::<Option<String>, _>("base_url")?.as_deref(),
                    row.try_get("api_key")?,
                    row.try_get("model")?,
                )?,
                row.try_get::<i64, _>("updated_at")?,
            )
        };
        let result = self.catalogue_call(&route, cancel).await;
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        self.authorize_byok_management(&*c, uid).await?;
        if matches!(result, Err(Error::Code("cancelled"))) {
            return Err(Error::Code("cancelled"));
        }
        let current = query("SELECT * FROM insight_user_byok WHERE user_id=?")
            .bind(uid)
            .fetch_optional(&*c)
            .await?
            .ok_or(Error::Code("credential_changed"))?;
        let current_route = provider::byok(
            &current.try_get::<String, _>("provider")?,
            current.try_get::<Option<String>, _>("base_url")?.as_deref(),
            current.try_get("api_key")?,
            current.try_get("model")?,
        )?;
        if current_route != route || current.try_get::<i64, _>("updated_at")? != revision {
            return Err(Error::Code("credential_changed"));
        }
        let error = if result.is_ok() {
            None
        } else {
            Some("provider_probe_failed")
        };
        let models = result.as_ref().ok().cloned().unwrap_or_default();
        let now = Utc::now();
        let ok = result.is_ok();
        let n=query("UPDATE insight_user_byok SET tested_at=CASE WHEN ? THEN ? ELSE tested_at END,test_error=?,models_json=CASE WHEN ? THEN ? ELSE models_json END,models_fetched_at=CASE WHEN ? THEN ? ELSE models_fetched_at END,updated_at=? WHERE user_id=? AND updated_at=? AND api_key=? AND model=?").bind(ok).bind(now).bind(error).bind(ok).bind(serde_json::to_string(&models).map_err(|_|invalid())?).bind(ok).bind(now).bind(now).bind(uid).bind(revision).bind(&route.key).bind(&route.model).execute(&*c).await?;
        if n.rows_affected() == 0 {
            return Err(Error::Code("credential_changed"));
        }
        drop(c);
        self.notify_access_change(Some(uid));
        Ok(json!({"ok":result.is_ok(),"error":error,"models":models}))
    }
    pub async fn admin_config(&self, uid: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, true).await?;
        self.config_on(&*c).await
    }
    async fn config_on<E: Executor>(&self, e: &E) -> Result<Value, Error> {
        Ok(
            json!({"allowed_models":serde_json::from_str::<Value>(&self.allowed_models(e).await?).unwrap_or(json!([])),"default_model":self.setting(e,"insight_tldr_default_model",&self.config.model).await?,"gateway_base_url":self.setting(e,"insight_tldr_gateway_url",&self.config.gateway_base).await?,"gateway_api_key":"","gateway_api_key_set":!self.setting(e,"insight_tldr_gateway_key",&self.config.gateway_key).await?.is_empty(),"openrouter_api_key":"","openrouter_api_key_set":!self.setting(e,"insight_tldr_openrouter_key",&self.config.openrouter_key).await?.is_empty(),"ai_route_mode":self.setting(e,"insight_tldr_route_mode",&self.config.mode).await?}),
        )
    }
    pub async fn update_config(&self, uid: i64, v: Value) -> Result<Value, Error> {
        let allowed: Vec<String> =
            serde_json::from_value(v["allowed_models"].clone()).map_err(|_| invalid())?;
        let default = v["default_model"].as_str().ok_or_else(invalid)?;
        if allowed.len() > 200
            || allowed.iter().any(|s| s.is_empty() || s.len() > 128)
            || !allowed.iter().any(|s| s == default)
            || !matches!(
                v["ai_route_mode"].as_str(),
                Some("auto" | "a2a" | "openrouter")
            )
        {
            return Err(invalid());
        }
        let base = v["gateway_base_url"].as_str().ok_or_else(invalid)?;
        let u = if base.is_empty() && v["ai_route_mode"] == "openrouter" {
            url::Url::parse("https://unused.invalid").map_err(|_| invalid())?
        } else {
            url::Url::parse(base).map_err(|_| invalid())?
        };
        if !matches!(u.scheme(), "http" | "https")
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
        {
            return Err(invalid());
        }
        let mut c = self.db.connection().await?;
        self.authorize(&*c, uid, None, true).await?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        let result = async {
            for (name, key) in [
                ("allowed_models", "insight_tldr_allowed_models"),
                ("default_model", "insight_tldr_default_model"),
                ("gateway_base_url", "insight_tldr_gateway_url"),
                ("gateway_api_key", "insight_tldr_gateway_key"),
                ("openrouter_api_key", "insight_tldr_openrouter_key"),
                ("ai_route_mode", "insight_tldr_route_mode"),
            ] {
                let value = if name == "allowed_models" {
                    v[name].to_string()
                } else {
                    v[name].as_str().unwrap_or("").into()
                };
                if (name == "gateway_api_key" || name == "openrouter_api_key")
                    && v.get(name).is_none()
                {
                    continue;
                }
                self.put_setting(&*tx, key, &value).await?
            }
            Ok::<_, Error>(())
        }
        .await;
        match result {
            Ok(()) => {
                tx.commit().await.map_err(|_| Error::Database)?;
                self.notify_access_change(None);
                let view = self.config_on(&*c).await;
                drop(c);
                view
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    pub async fn byok_toggle(&self, uid: i64, enabled: Option<bool>) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, true).await?;
        if let Some(enabled) = enabled {
            self.put_setting(
                &*c,
                "insight_byok_enabled",
                if enabled { "true" } else { "false" },
            )
            .await?
        }
        if enabled.is_some() {
            self.notify_access_change(None);
        }
        let view = json!({"enabled":self.byok_enabled(&*c).await?});
        drop(c);
        Ok(view)
    }
    pub async fn access_list(&self, actor: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, actor, None, true).await?;
        self.access_on(&*c).await
    }
    async fn access_on<E: Executor>(&self, e: &E) -> Result<Value, Error> {
        let rows=query("SELECT a.*,u.username,u.first_name,g.username AS granted_by_username FROM insight_access a LEFT JOIN users u ON u.id=a.user_id LEFT JOIN users g ON g.id=a.granted_by ORDER BY a.granted_at ASC").fetch_all(e).await?;
        let mut list = Vec::new();
        for r in rows {
            list.push(json!({"user_id":r.try_get::<i64,_>("user_id")?,"lang":r.try_get::<String,_>("lang")?,"granted_by":r.try_get::<i64,_>("granted_by")?,"granted_at":r.try_get::<chrono::DateTime<Utc>,_>("granted_at")?,"username":r.try_get::<Option<String>,_>("username")?,"first_name":r.try_get::<Option<String>,_>("first_name")?,"granted_by_username":r.try_get::<Option<String>,_>("granted_by_username")?}));
        }
        Ok(json!(list))
    }
    pub async fn grant(&self, actor: i64, uid: i64, lang: &str) -> Result<Value, Error> {
        if uid <= 0 || !valid_language(lang) {
            return Err(invalid());
        }
        let mut c = self.db.connection().await?;
        self.authorize(&*c, actor, None, true).await?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        let result=async{if query("SELECT 1 FROM users WHERE id=?").bind(uid).fetch_optional(&*tx).await?.is_none(){return Err(Error::Code("not_found"));}query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES(?,'insight','summary',?,?) ON CONFLICT(user_id,plugin,feature) DO UPDATE SET granted_by=excluded.granted_by,granted_at=excluded.granted_at,expires_at=NULL").bind(uid).bind(actor).bind(Utc::now()).execute(&*tx).await?;query("INSERT INTO insight_access(user_id,lang,granted_by,granted_at) VALUES(?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET lang=excluded.lang,granted_by=excluded.granted_by,granted_at=excluded.granted_at").bind(uid).bind(lang).bind(actor).bind(Utc::now()).execute(&*tx).await?;query("INSERT INTO insight_user_settings(user_id,lang) VALUES(?,?) ON CONFLICT(user_id) DO UPDATE SET lang=excluded.lang").bind(uid).bind(lang).execute(&*tx).await?;Ok::<_,Error>(())}.await;
        match result {
            Ok(()) => {
                tx.commit().await.map_err(|_| Error::Database)?;
                self.notify_access_change(Some(uid));
                let list = self.access_on(&*c).await;
                drop(c);
                let list = list?;
                Ok(list
                    .as_array()
                    .and_then(|a| a.iter().find(|v| v["user_id"] == uid))
                    .cloned()
                    .unwrap_or(Value::Null))
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    pub async fn revoke(&self, actor: i64, uid: i64) -> Result<(), Error> {
        let mut c = self.db.connection().await?;
        self.authorize(&*c, actor, None, true).await?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        let result=async{if query("SELECT 1 FROM insight_access WHERE user_id=?").bind(uid).fetch_optional(&*tx).await?.is_none(){return Err(Error::Code("not_found"));}query("DELETE FROM user_permissions WHERE user_id=? AND plugin='insight' AND feature='summary'").bind(uid).execute(&*tx).await?;query("DELETE FROM insight_access WHERE user_id=?").bind(uid).execute(&*tx).await?;Ok::<_,Error>(())}.await;
        match result {
            Ok(()) => {
                tx.commit().await.map_err(|_| Error::Database)?;
                drop(c);
                self.notify_access_change(Some(uid));
                Ok(())
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    pub async fn lookup(&self, actor: i64, q: &str) -> Result<Value, Error> {
        let q = q.trim_start_matches('@');
        if q.is_empty() || q.len() > 128 {
            return Err(invalid());
        }
        let c = self.db.connection().await?;
        self.authorize(&*c, actor, None, true).await?;
        let rows=query("SELECT id,username,first_name FROM users WHERE ((?=0 AND username LIKE ? COLLATE NOCASE) OR (?=1 AND CAST(id AS TEXT)=?)) LIMIT 20").bind(q.bytes().all(|b| b.is_ascii_digit())).bind(format!("%{q}%")).bind(q.bytes().all(|b| b.is_ascii_digit())).bind(q).fetch_all(&*c).await?;
        let mut list = Vec::new();
        for r in rows {
            list.push(json!({"id":r.try_get::<i64,_>("id")?,"username":r.try_get::<Option<String>,_>("username")?,"first_name":r.try_get::<Option<String>,_>("first_name")?}))
        }
        Ok(json!(list))
    }
}
#[derive(Deserialize)]
pub struct ByokTest {
    pub provider: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}
impl Insight {
    pub async fn test_byok_input(
        &self,
        uid: i64,
        mut v: ByokTest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        v.base_url = v
            .base_url
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        v.api_key = v.api_key.map(|s| s.trim().to_owned());
        let (key, stored_revision) = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, None, false).await?;
            self.authorize_byok_management(&*c, uid).await?;
            if v.api_key.as_deref() == Some("") {
                return Ok(json!({"ok":false,"error":"api_key_required","models":[]}));
            }
            if let Some(key) = v.api_key {
                (key, None)
            } else {
                let Some(r) = query("SELECT * FROM insight_user_byok WHERE user_id=?")
                    .bind(uid)
                    .fetch_optional(&*c)
                    .await?
                else {
                    return Ok(json!({"ok":false,"error":"api_key_required","models":[]}));
                };
                (
                    r.try_get("api_key")?,
                    Some(r.try_get::<i64, _>("updated_at")?),
                )
            }
        };
        let route = provider::byok(&v.provider, v.base_url.as_deref(), key, "probe".into())?;
        let result = self.catalogue_call(&route, cancel).await;
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        self.authorize_byok_management(&*c, uid).await?;
        if let Some(revision) = stored_revision {
            let current = query(
                "SELECT 1 FROM insight_user_byok WHERE user_id=? AND updated_at=? AND api_key=?",
            )
            .bind(uid)
            .bind(revision)
            .bind(&route.key)
            .fetch_optional(&*c)
            .await?;
            if current.is_none() {
                return Err(Error::Code("credential_changed"));
            }
        }
        match result {
            Ok(models) => Ok(json!({"ok":true,"error":null,"models":models})),
            Err(Error::Code("cancelled")) => Err(Error::Code("cancelled")),
            Err(_) => Ok(json!({"ok":false,"error":"provider_probe_failed","models":[]})),
        }
    }
    pub async fn refresh_byok_models(
        &self,
        uid: i64,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        self.probe_byok(uid, cancel).await?;
        self.byok_view(uid).await
    }
    /// Current configured owner or current DB owner role, never a JWT role.
    pub(crate) async fn is_owner<E: Executor>(&self, e: &E, uid: i64) -> Result<bool, Error> {
        self.authorize(e, uid, None, false).await?;
        let role: String = query("SELECT role FROM users WHERE id=?")
            .bind(uid)
            .fetch_one(e)
            .await?
            .try_get("role")?;
        Ok(role == "owner" || self.config.owner_id == Some(uid))
    }
    async fn authorize_byok_management<E: Executor>(&self, e: &E, uid: i64) -> Result<(), Error> {
        self.authorize(e, uid, None, false).await?;
        if !self.byok_enabled(e).await? && !self.is_owner(e, uid).await? {
            return Err(Error::Code("byok_disabled"));
        }
        Ok(())
    }
    async fn catalogue_call(
        &self,
        route: &provider::Route,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Vec<provider::Model>, Error> {
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(Error::Code("cancelled")),
            result = tokio::time::timeout(self.config.request_timeout, self.completion.models(route, cancel.clone())) => result.unwrap_or(Err(Error::Code("provider_unavailable"))),
        }
    }
    pub async fn models(
        &self,
        uid: i64,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        self.models_with_options(uid, ModelProbeOptions::default(), cancel)
            .await
    }
    pub async fn models_with_options(
        &self,
        uid: i64,
        options: ModelProbeOptions,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        let route = {
            let c = self.db.connection().await?;
            if !self.is_owner(&*c, uid).await? {
                // Legacy deliberately ignores even invalid overrides for nonowners.
                let allowed: Vec<String> = serde_json::from_str(&self.allowed_models(&*c).await?)
                    .map_err(|_| invalid())?;
                return Ok(json!(
                    allowed
                        .into_iter()
                        .map(|id| json!({"id":id}))
                        .collect::<Vec<_>>()
                ));
            }
            self.model_probe_route(&*c, &options).await?
        };
        let result = self.catalogue_call(&route, cancel).await;
        let c = self.db.connection().await?;
        if !self.is_owner(&*c, uid).await? {
            return Err(Error::Code("access_denied"));
        }
        if self.model_probe_route(&*c, &options).await? != route {
            return Err(Error::Code("route_changed"));
        }
        match result {
            Ok(models) => Ok(json!(models)),
            Err(Error::Code("cancelled")) => Err(Error::Code("cancelled")),
            Err(_) => Ok(json!([{"id":self.config.model}])), // pinned fallback
        }
    }
    async fn model_probe_route<E: Executor>(
        &self,
        e: &E,
        options: &ModelProbeOptions,
    ) -> Result<provider::Route, Error> {
        let mut route = self.gateway_route(e).await?;
        if let Some(base) = options.url.as_deref().filter(|s| !s.is_empty()) {
            route.base = base.trim_end_matches('/').into();
            // URL override with omitted key means anonymous, not the stored key.
            route.key = options.api_key.clone().unwrap_or_default();
        }
        let url = url::Url::parse(&route.base).map_err(|_| invalid())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || route.base.len() > 2048
            || route.key.len() > 8192
        {
            return Err(invalid());
        }
        route.base = route.base.trim_end_matches('/').into();
        if !route.base.ends_with("/v1") {
            route.base.push_str("/v1");
        }
        Ok(route)
    }
    pub(crate) async fn gateway_route<E: Executor>(&self, e: &E) -> Result<provider::Route, Error> {
        Ok(provider::Route {
            shape: provider::Shape::OpenAi,
            base: self
                .setting(e, "insight_tldr_gateway_url", &self.config.gateway_base)
                .await?,
            key: self
                .setting(e, "insight_tldr_gateway_key", &self.config.gateway_key)
                .await?,
            model: self
                .setting(e, "insight_tldr_default_model", &self.config.model)
                .await?,
            via: "gateway",
        })
    }
    pub async fn test_gateway(
        &self,
        uid: i64,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        self.test_gateway_with_options(uid, ModelProbeOptions::default(), cancel)
            .await
    }
    pub async fn test_gateway_with_options(
        &self,
        uid: i64,
        options: ModelProbeOptions,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Value, Error> {
        let (route, display_url) = {
            let c = self.db.connection().await?;
            self.authorize(&*c, uid, None, true).await?;
            let display_url = options
                .url
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_owned())
                .unwrap_or(
                    self.gateway_route(&*c)
                        .await?
                        .base
                        .trim_end_matches('/')
                        .into(),
                );
            (self.model_probe_route(&*c, &options).await?, display_url)
        };
        let result = self.catalogue_call(&route, cancel).await;
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, true).await?;
        if self.model_probe_route(&*c, &options).await? != route {
            return Err(Error::Code("route_changed"));
        }
        Ok(match result {
            Ok(models) => json!({"ok":true,"model_count":models.len(),"url":display_url}),
            Err(Error::Code("cancelled")) => return Err(Error::Code("cancelled")),
            Err(_) => json!({"ok":false,"error":"provider_probe_failed","url":display_url}),
        })
    }
    pub async fn write_token_status(&self, uid: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        let row =
            query("SELECT github_token_public_repo FROM insight_user_settings WHERE user_id=?")
                .bind(uid)
                .fetch_optional(&*c)
                .await?;
        let token = row
            .map(|r| r.try_get::<Option<String>, _>("github_token_public_repo"))
            .transpose()?
            .flatten();
        Ok(json!({"enabled":token.is_some()}))
    }

    pub async fn activity(&self, uid: i64) -> Result<Value, Error> {
        let c = self.db.connection().await?;
        self.authorize(&*c, uid, None, false).await?;
        let r=query("SELECT count(*) AS total,max(created_at) AS last_at FROM insight_usage_log WHERE user_id=?").bind(uid).fetch_one(&*c).await?;
        Ok(
            json!({"plugin":"insight","total":r.try_get::<i64,_>("total")?,"last_at":r.try_get::<Option<chrono::DateTime<Utc>>,_>("last_at")?,"extra":{}}),
        )
    }
}

fn nullable_model<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

/// Frontend query contract: url and api_key; overrides are never persisted.
#[derive(Default, Deserialize)]
pub struct ModelProbeOptions {
    pub url: Option<String>,
    pub api_key: Option<String>,
}

// Kept private: this revision includes credentials and must never be logged.
type ByokRevision = (String, Option<String>, String, String, i64);

pub(crate) fn valid_language(lang: &str) -> bool {
    matches!(lang, "en" | "ru" | "uk" | "de" | "fr" | "es" | "zh" | "ja")
}

impl Insight {
    /// PATCH changes the existing access row only, never grant provenance.
    pub async fn update_access(&self, actor: i64, uid: i64, lang: &str) -> Result<Value, Error> {
        if !valid_language(lang) {
            return Err(invalid());
        }
        let c = self.db.connection().await?;
        self.authorize(&*c, actor, None, true).await?;
        let n = query("UPDATE insight_access SET lang=? WHERE user_id=?")
            .bind(lang)
            .bind(uid)
            .execute(&*c)
            .await?;
        if n.rows_affected() == 0 {
            return Err(Error::Code("not_found"));
        }
        Ok(self
            .access_on(&*c)
            .await?
            .as_array()
            .and_then(|a| a.iter().find(|v| v["user_id"] == uid))
            .cloned()
            .unwrap_or(Value::Null))
    }
    pub(crate) async fn allowed_models<E: Executor>(&self, e: &E) -> Result<String, Error> {
        let fallback =
            serde_json::to_string(&vec![self.config.model.clone()]).map_err(|_| invalid())?;
        self.setting(e, "insight_tldr_allowed_models", &fallback)
            .await
    }
}
