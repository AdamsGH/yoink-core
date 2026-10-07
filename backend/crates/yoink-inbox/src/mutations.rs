//! Crash-durable external write coordinator. A sending/unknown intent is never
//! resent. Local effects and `applied` commit together. No credentials are stored.
use crate::{
    Error, Result,
    contracts::{Authorization, Insight},
    github::Github,
    lists,
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yoink_db::{Executor, query};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Mutation {
    CreateList {
        body: Value,
    },
    EditList {
        folder_id: i64,
        body: Value,
    },
    DeleteList {
        folder_id: i64,
    },
    Membership {
        folder_id: i64,
        star_id: i64,
        add: bool,
    },
    Star {
        star_id: i64,
        add: bool,
    },
}
/// Complete model for sql/github_mutations.sql. Debug deliberately omits payloads.
#[derive(Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: String,
    pub user_id: i64,
    pub request_key: Option<String>,
    pub request: Mutation,
    pub state: String,
    pub prepared: Option<Value>,
    pub result: Option<Value>,
    pub send_token: Option<String>,
    pub local_folder_id: Option<i64>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
fn decode(r: yoink_db::Row) -> Result<Intent> {
    Ok(Intent {
        id: r.try_get("id")?,
        user_id: r.try_get("user_id")?,
        request_key: r.try_get("request_key")?,
        request: serde_json::from_value(r.try_get("request")?).map_err(|_| Error::Invalid)?,
        state: r.try_get("state")?,
        prepared: r.try_get("prepared")?,
        result: r.try_get("result")?,
        send_token: r.try_get("send_token")?,
        local_folder_id: r.try_get("local_folder_id")?,
        error_code: r.try_get("error_code")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or(Error::Invalid)
}
fn optional_text(v: &Value, k: &str, max: usize) -> Result<Option<String>> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.chars().count() <= max => Ok(Some(s.clone())),
        _ => Err(Error::Invalid),
    }
}
fn validate_body(v: &Value, create: bool) -> Result<()> {
    if !v.is_object() || v.get("gh_list_id").is_some() || v.get("gh_list_slug").is_some() {
        return Err(Error::Invalid);
    }
    if create || v.get("name").is_some() {
        let s = text(v, "name")?;
        if s.is_empty() || s.chars().count() > 128 {
            return Err(Error::Invalid);
        }
    }
    optional_text(v, "slug", 64)?;
    optional_text(v, "icon", 32)?;
    optional_text(v, "description", usize::MAX)?;
    if v.get("sort_order")
        .is_some_and(|n| n.as_i64().is_none_or(|n| i32::try_from(n).is_err()))
        || v.get("is_pinned").is_some_and(|n| !n.is_boolean())
        || v.get("parent_id")
            .is_some_and(|n| !n.is_null() && n.as_i64().is_none())
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
async fn parent<E: Executor>(c: &E, user: i64, id: Option<i64>, p: Option<i64>) -> Result<()> {
    let mut p = p;
    let mut visited = std::collections::HashSet::new();
    while let Some(n) = p {
        if Some(n) == id || !visited.insert(n) {
            return Err(Error::Invalid);
        }
        p = query("SELECT parent_id FROM inbox_gh_folders WHERE user_id=? AND id=?")
            .bind(user)
            .bind(n)
            .fetch_optional(c)
            .await?
            .ok_or(Error::NotFound)?
            .try_get("parent_id")?;
    }
    Ok(())
}
impl Store {
    pub async fn mutation(&self, user: i64, id: &str) -> Result<Intent> {
        decode(
            query("SELECT * FROM inbox_gh_mutations WHERE user_id=? AND id=?")
                .bind(user)
                .bind(id)
                .fetch_optional(&self.db)
                .await?
                .ok_or(Error::NotFound)?,
        )
    }
    pub async fn active_mutations(&self, user: i64) -> Result<Vec<Intent>> {
        query("SELECT * FROM inbox_gh_mutations WHERE user_id=? AND state IN ('prepared','sending','unknown','confirmed') ORDER BY created_at,id").bind(user).fetch_all(&self.db).await?.into_iter().map(decode).collect()
    }
    /// Admission reserves a local create and its journal in one transaction. The
    /// active unique index blocks a different concurrent request for this user.
    pub async fn prepare_mutation(
        &self,
        user: i64,
        request: &Mutation,
        now: i64,
    ) -> Result<Intent> {
        self.prepare_mutation_keyed(user, request, None, now).await
    }
    pub async fn prepare_mutation_keyed(
        &self,
        user: i64,
        request: &Mutation,
        key: Option<&str>,
        now: i64,
    ) -> Result<Intent> {
        if key.is_some_and(|k| k.is_empty() || k.len() > 128) {
            return Err(Error::Invalid);
        }
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
 if let Some(key)=key && let Some(r)=query("SELECT * FROM inbox_gh_mutations WHERE user_id=? AND request_key=?").bind(user).bind(key).fetch_optional(&*c).await? {let prior=decode(r)?;return if prior.request==*request {Ok(prior.id)}else{Err(Error::Invalid)};}
 if let Some(r)=query("SELECT * FROM inbox_gh_mutations WHERE user_id=? AND state IN ('prepared','sending','unknown','confirmed')").bind(user).fetch_optional(&*c).await?{let prior=decode(r)?;return if prior.request==*request{Ok(prior.id)}else{Err(Error::MutationPending(prior.id))};}
 let mut folder=None;let mut prepared=json!({});
 match request {
 Mutation::CreateList{body}=>{validate_body(body,true)?;parent(&*c,user,None,body["parent_id"].as_i64()).await?;
 let name=text(body,"name")?;let base=optional_text(body,"slug",64)?.filter(|s|!s.is_empty()).unwrap_or_else(||{let s=crate::normalize::slug(name);if s.is_empty(){"folder".into()}else{s}});let slug=crate::normalize::unique_slug(&*c,"inbox_gh_folders","user_id",user,&base,None).await?;
 folder=Some(query("INSERT INTO inbox_gh_folders(user_id,name,slug,description,icon,parent_id,is_pinned,sort_order,created_at) VALUES(?,?,?,?,?,?,?,?,?) RETURNING id").bind(user).bind(name).bind(slug).bind(optional_text(body,"description",usize::MAX)?).bind(optional_text(body,"icon",32)?).bind(body["parent_id"].as_i64()).bind(body["is_pinned"].as_bool().unwrap_or(false)).bind(body["sort_order"].as_i64().unwrap_or(0)).bind(now).fetch_one(&*c).await?.try_get::<i64,_>("id")?);},
 Mutation::EditList{folder_id,body}=>{validate_body(body,false)?;parent(&*c,user,Some(*folder_id),body["parent_id"].as_i64()).await?;let r=query("SELECT gh_list_id FROM inbox_gh_folders WHERE user_id=? AND id=?").bind(user).bind(*folder_id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let remote:Option<String>=r.try_get("gh_list_id")?;prepared["list_id"]=json!(remote.ok_or(Error::Invalid)?);folder=Some(*folder_id);},
 Mutation::DeleteList{folder_id}=>{let r=query("SELECT gh_list_id FROM inbox_gh_folders WHERE user_id=? AND id=?").bind(user).bind(*folder_id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let remote:Option<String>=r.try_get("gh_list_id")?;prepared["list_id"]=json!(remote.ok_or(Error::Invalid)?);folder=Some(*folder_id);},
 Mutation::Membership{folder_id,star_id,..}=>{let r=query("SELECT gh_list_id FROM inbox_gh_folders WHERE user_id=? AND id=?").bind(user).bind(*folder_id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let remote:Option<String>=r.try_get("gh_list_id")?;let s=query("SELECT gh_node_id FROM inbox_gh_stars WHERE user_id=? AND id=?").bind(user).bind(*star_id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let node:Option<String>=s.try_get("gh_node_id")?;prepared=json!({"list_id":remote.ok_or(Error::Invalid)?,"node_id":node.ok_or(Error::Invalid)?});folder=Some(*folder_id);},
 Mutation::Star{star_id,..}=>{let r=query("SELECT full_name FROM inbox_gh_stars WHERE user_id=? AND id=?").bind(user).bind(*star_id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let name:String=r.try_get("full_name")?;let(owner,repo)=name.split_once('/').ok_or(Error::Invalid)?;prepared=json!({"owner":owner,"repo":repo});}
 }
 let id=uuid::Uuid::new_v4().to_string();let request=serde_json::to_value(request).map_err(|_|Error::Invalid)?;
 query("INSERT INTO inbox_gh_mutations(id,user_id,request_key,request,state,prepared,local_folder_id,created_at,updated_at) VALUES(?,?,?,?,'prepared',?,?,?,?)").bind(&id).bind(user).bind(key).bind(request).bind(prepared).bind(folder).bind(now).bind(now).execute(&*c).await?;Ok(id)
 }.await;
        let id = match result {
            Ok(id) => {
                c.batch("COMMIT").await?;
                id
            }
            Err(e) => {
                c.batch("ROLLBACK").await?;
                return Err(e);
            }
        };
        drop(c);
        self.mutation(user, &id).await
    }
    /// Durable send boundary. CAS guarantees no automatic replay even after crash.
    pub async fn begin_mutation_send(
        &self,
        intent: &Intent,
        prepared: &Value,
        now: i64,
    ) -> Result<bool> {
        let token = uuid::Uuid::new_v4().to_string();
        Ok(query("UPDATE inbox_gh_mutations SET state='sending',prepared=?,send_token=?,updated_at=? WHERE id=? AND user_id=? AND state='prepared'").bind(prepared).bind(token).bind(now).bind(&intent.id).bind(intent.user_id).execute(&self.db).await?.rows_affected()==1)
    }
    pub async fn confirm_mutation(
        &self,
        user: i64,
        id: &str,
        result: &Value,
        now: i64,
    ) -> Result<()> {
        query("UPDATE inbox_gh_mutations SET state='confirmed',result=?,error_code=NULL,updated_at=? WHERE id=? AND user_id=? AND state IN ('sending','unknown')").bind(result).bind(now).bind(id).bind(user).execute(&self.db).await?;
        Ok(())
    }
    pub async fn unknown_mutation(&self, user: i64, id: &str, now: i64) -> Result<()> {
        query("UPDATE inbox_gh_mutations SET state='unknown',error_code='outcome_unknown',updated_at=? WHERE id=? AND user_id=? AND state='sending'").bind(now).bind(id).bind(user).execute(&self.db).await?;
        Ok(())
    }
    /// Cancel only an intent proven not to have crossed the send boundary.
    pub async fn cancel_prepared_mutation(&self, user: i64, id: &str, now: i64) -> Result<()> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
 let r=query("SELECT request,local_folder_id FROM inbox_gh_mutations WHERE user_id=? AND id=? AND state='prepared'").bind(user).bind(id).fetch_optional(&*c).await?.ok_or(Error::Uncertain)?;let request:Value=r.try_get("request")?;if request["type"]=="create_list"{query("DELETE FROM inbox_gh_folders WHERE user_id=? AND id=?").bind(user).bind(r.try_get::<Option<i64>,_>("local_folder_id")?).execute(&*c).await?;}
 query("UPDATE inbox_gh_mutations SET state='rejected',send_token=?,error_code='cancelled_before_send',updated_at=? WHERE id=? AND user_id=? AND state='prepared'").bind(uuid::Uuid::new_v4().to_string()).bind(now).bind(id).bind(user).execute(&*c).await?;Ok(())}.await;
        match result {
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
    pub async fn apply_mutation(&self, user: i64, id: &str, now: i64) -> Result<Intent> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
 let intent=decode(query("SELECT * FROM inbox_gh_mutations WHERE user_id=? AND id=?").bind(user).bind(id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?)?;
 if intent.state=="applied"{return Ok(())}
 if intent.state!="confirmed"{return Err(Error::MutationPending(id.into()))}
 let prepared=intent.prepared.as_ref().ok_or(Error::Invalid)?;let remote=intent.result.as_ref().ok_or(Error::Invalid)?;
 match &intent.request {
 Mutation::CreateList{..}=>{let folder=intent.local_folder_id.ok_or(Error::Uncertain)?;
 if let Some(existing)=query("SELECT id FROM inbox_gh_folders WHERE user_id=? AND gh_list_id=? AND id<>?").bind(user).bind(text(remote,"id")?).bind(folder).fetch_optional(&*c).await?{
 // Read-only list sync may already have imported the explicitly adopted
 // candidate. Reuse it, retaining its metadata/pins and manual memberships.
 let existing=existing.try_get::<i64,_>("id")?;
 query("INSERT INTO inbox_gh_folder_members(folder_id,gh_star_id,added_by,added_at) SELECT ?,gh_star_id,added_by,added_at FROM inbox_gh_folder_members WHERE folder_id=? ON CONFLICT DO NOTHING").bind(existing).bind(folder).execute(&*c).await?;
 query("UPDATE inbox_gh_folders SET parent_id=? WHERE user_id=? AND parent_id=? AND id<>?").bind(existing).bind(user).bind(folder).bind(existing).execute(&*c).await?;
 query("UPDATE inbox_gh_mutations SET local_folder_id=? WHERE user_id=? AND id=?").bind(existing).bind(user).bind(id).execute(&*c).await?;
 query("DELETE FROM inbox_gh_folders WHERE user_id=? AND id=?").bind(user).bind(folder).execute(&*c).await?;
 }else if query("UPDATE inbox_gh_folders SET gh_list_id=?,gh_list_slug=? WHERE user_id=? AND id=?").bind(text(remote,"id")?).bind(remote["slug"].as_str()).bind(user).bind(folder).execute(&*c).await?.rows_affected()!=1{return Err(Error::Uncertain)}},
 Mutation::EditList{folder_id,body}=>{parent(&*c,user,Some(*folder_id),body["parent_id"].as_i64()).await?;
 let row=query("SELECT * FROM inbox_gh_folders WHERE user_id=? AND id=? AND gh_list_id=?").bind(user).bind(*folder_id).bind(text(prepared,"list_id")?).fetch_optional(&*c).await?.ok_or(Error::Uncertain)?;
 let name=body["name"].as_str().map(str::to_owned).unwrap_or(row.try_get("name")?);
 let desc=if body.get("description").is_some(){optional_text(body,"description",usize::MAX)?}else{row.try_get("description")?};let icon=if body.get("icon").is_some(){optional_text(body,"icon",usize::MAX)?}else{row.try_get("icon")?};
 let pinned=body["is_pinned"].as_bool().unwrap_or(row.try_get("is_pinned")?);let order=body["sort_order"].as_i64().unwrap_or(row.try_get("sort_order")?);let p=if body.get("parent_id").is_some(){body["parent_id"].as_i64()}else{row.try_get("parent_id")?};
 query("UPDATE inbox_gh_folders SET name=?,description=?,icon=?,parent_id=?,is_pinned=?,sort_order=? WHERE id=? AND user_id=?").bind(name).bind(desc).bind(icon).bind(p).bind(pinned).bind(order).bind(*folder_id).bind(user).execute(&*c).await?;},
 Mutation::DeleteList{folder_id}=>{query("DELETE FROM inbox_gh_folders WHERE id=? AND user_id=? AND gh_list_id=?").bind(*folder_id).bind(user).bind(text(prepared,"list_id")?).execute(&*c).await?;},
 Mutation::Membership{folder_id,star_id,add}=>{query("SELECT id FROM inbox_gh_folders WHERE user_id=? AND id=? AND gh_list_id=?").bind(user).bind(*folder_id).bind(text(prepared,"list_id")?).fetch_optional(&*c).await?.ok_or(Error::Uncertain)?;query("SELECT id FROM inbox_gh_stars WHERE user_id=? AND id=? AND gh_node_id=?").bind(user).bind(*star_id).bind(text(prepared,"node_id")?).fetch_optional(&*c).await?.ok_or(Error::Uncertain)?;
 if *add {query("INSERT INTO inbox_gh_folder_members(folder_id,gh_star_id,added_by,added_at) VALUES(?,?,'user',?) ON CONFLICT DO NOTHING").bind(*folder_id).bind(*star_id).bind(now).execute(&*c).await?;}else{query("DELETE FROM inbox_gh_folder_members WHERE folder_id=? AND gh_star_id=?").bind(*folder_id).bind(*star_id).execute(&*c).await?;}},
 Mutation::Star{star_id,add}=>{let name=format!("{}/{}",text(prepared,"owner")?,text(prepared,"repo")?);query("UPDATE inbox_gh_stars SET can_unstar=? WHERE id=? AND user_id=? AND full_name=?").bind(*add).bind(*star_id).bind(user).bind(name).execute(&*c).await?;}
 }
 query("UPDATE inbox_gh_mutations SET state='applied',updated_at=? WHERE id=? AND user_id=? AND state='confirmed'").bind(now).bind(id).bind(user).execute(&*c).await?;Ok(())
 }.await;
        match result {
            Ok(()) => c.batch("COMMIT").await?,
            Err(e) => {
                c.batch("ROLLBACK").await?;
                return Err(e);
            }
        }
        drop(c);
        self.mutation(user, id).await
    }
}
pub struct Coordinator<'a> {
    pub store: &'a Store,
    pub auth: &'a dyn Authorization,
    pub insight: &'a dyn Insight,
    pub github: &'a dyn Github,
}
impl Coordinator<'_> {
    async fn authorize(&self, user: i64) -> Result<String> {
        self.auth.require(user, "inbox:ingest").await?;
        self.auth.require(user, "inbox:gh_write").await?;
        self.insight
            .github_token(user, true)
            .await?
            .ok_or(Error::Forbidden)
    }
    pub async fn execute(
        &self,
        user: i64,
        request: &Mutation,
        clock: impl Fn() -> i64,
    ) -> Result<Intent> {
        self.execute_keyed(user, request, None, clock).await
    }
    pub async fn execute_keyed(
        &self,
        user: i64,
        request: &Mutation,
        key: Option<&str>,
        clock: impl Fn() -> i64,
    ) -> Result<Intent> {
        let _ = self.authorize(user).await?;
        let intent = self
            .store
            .prepare_mutation_keyed(user, request, key, clock())
            .await?;
        self.drive(user, &intent.id, clock).await
    }
    pub async fn drive(&self, user: i64, id: &str, clock: impl Fn() -> i64) -> Result<Intent> {
        let token = self.authorize(user).await?;
        let intent = self.store.mutation(user, id).await?;
        match intent.state.as_str() {
            "applied" => return Ok(intent),
            "confirmed" => return self.store.apply_mutation(user, id, clock()).await,
            "sending" | "unknown" => return self.reconcile(user, id, clock).await,
            "prepared" => (),
            _ => return Err(Error::Invalid),
        }
        let mut prepared = intent.prepared.clone().ok_or(Error::Invalid)?;
        match &intent.request {
            Mutation::CreateList { .. } => {
                let catalog = lists::catalogue(self.github, &token).await?;
                prepared["baseline"] = json!(catalog.iter().map(|l| &l.id).collect::<Vec<_>>());
            }
            Mutation::Membership { add, .. } => {
                let mut ids =
                    lists::membership_ids(self.github, &token, text(&prepared, "node_id")?).await?;
                let target = text(&prepared, "list_id")?;
                if *add {
                    if !ids.iter().any(|i| i == target) {
                        ids.push(target.into())
                    }
                } else {
                    ids.retain(|i| i != target)
                }
                prepared["desired_ids"] = json!(ids);
            }
            _ => (),
        }
        // Recheck bans/grants after preflight and immediately before durable send admission.
        let token = self.authorize(user).await?;
        if !self
            .store
            .begin_mutation_send(&intent, &prepared, clock())
            .await?
        {
            return Err(Error::MutationPending(id.into()));
        }
        let outcome = match &intent.request {
            Mutation::CreateList { body } => {
                lists::create(
                    self.github,
                    &token,
                    text(body, "name")?,
                    body["description"].as_str().unwrap_or(""),
                )
                .await
            }
            Mutation::EditList { body, .. } => {
                if let Some(name) = body["name"].as_str() {
                    lists::rename(self.github, &token, text(&prepared, "list_id")?, name)
                        .await
                        .map(|()| json!({}))
                } else {
                    Ok(json!({}))
                }
            }
            Mutation::DeleteList { .. } => {
                lists::delete(self.github, &token, text(&prepared, "list_id")?)
                    .await
                    .map(|()| json!({}))
            }
            Mutation::Membership { .. } => {
                let ids: Vec<String> = serde_json::from_value(prepared["desired_ids"].clone())
                    .map_err(|_| Error::Invalid)?;
                lists::replace_memberships(self.github, &token, text(&prepared, "node_id")?, &ids)
                    .await
                    .map(|()| json!({}))
            }
            Mutation::Star { add, .. } => self
                .github
                .star(
                    &token,
                    text(&prepared, "owner")?,
                    text(&prepared, "repo")?,
                    *add,
                )
                .await
                .map(|()| json!({})),
        };
        match outcome {
            Ok(result) => {
                self.store
                    .confirm_mutation(user, id, &result, clock())
                    .await?
            }
            Err(e @ (Error::Forbidden | Error::RateLimited)) => {
                self.store
                    .reject_mutation(
                        user,
                        id,
                        if matches!(e, Error::Forbidden) {
                            "auth_failed"
                        } else {
                            "rate_limited"
                        },
                        clock(),
                    )
                    .await?;
                return Err(e);
            }
            Err(_) => {
                self.store.unknown_mutation(user, id, clock()).await?;
                return Err(Error::MutationPending(id.into()));
            }
        }
        self.store.apply_mutation(user, id, clock()).await
    }
    /// Recovery performs reads only. Observing the desired effect is sufficient
    /// for idempotent edits/membership/star state. Absence never permits resending.
    pub async fn reconcile(&self, user: i64, id: &str, clock: impl Fn() -> i64) -> Result<Intent> {
        let token = self.authorize(user).await?;
        let intent = self.store.mutation(user, id).await?;
        if intent.state == "confirmed" {
            return self.store.apply_mutation(user, id, clock()).await;
        }
        if intent.state == "applied" {
            return Ok(intent);
        }
        if !["sending", "unknown"].contains(&intent.state.as_str()) {
            return Err(Error::Invalid);
        }
        let prepared = intent.prepared.as_ref().ok_or(Error::Invalid)?;
        let matches = match &intent.request {
            Mutation::CreateList { .. } => false, // No observable clientMutationId: requires explicit adoption.
            Mutation::EditList { body, .. } => {
                let d = self
                    .github
                    .graphql(
                        &token,
                        "query($id:ID!){node(id:$id){... on UserList{id name}}}",
                        json!({"id":text(prepared,"list_id")?}),
                        false,
                    )
                    .await?;
                body["name"].as_str().is_none_or(|n| d["node"]["name"] == n)
            }
            Mutation::DeleteList { .. } => {
                let catalog = lists::catalogue(self.github, &token).await?;
                !catalog
                    .iter()
                    .any(|l| Some(l.id.as_str()) == prepared["list_id"].as_str())
            }
            Mutation::Membership { add, .. } => {
                let ids =
                    lists::membership_ids(self.github, &token, text(prepared, "node_id")?).await?;
                ids.iter()
                    .any(|i| Some(i.as_str()) == prepared["list_id"].as_str())
                    == *add
            }
            Mutation::Star { add, .. } => {
                self.github
                    .is_starred(&token, text(prepared, "owner")?, text(prepared, "repo")?)
                    .await?
                    == *add
            }
        };
        if matches {
            self.store
                .confirm_mutation(user, id, &json!({}), clock())
                .await?;
            self.store.apply_mutation(user, id, clock()).await
        } else {
            self.store.unknown_mutation(user, id, clock()).await?;
            Err(Error::MutationPending(id.into()))
        }
    }
    /// Explicit user-authorized adoption after an ambiguous create. Never guess
    /// ownership from a name alone; present candidate IDs to the caller first.
    pub async fn create_candidates(&self, user: i64, id: &str) -> Result<Vec<Value>> {
        let token = self.authorize(user).await?;
        let intent = self.store.mutation(user, id).await?;
        let Mutation::CreateList { body } = intent.request else {
            return Err(Error::Invalid);
        };
        if !["sending", "unknown"].contains(&intent.state.as_str()) {
            return Err(Error::Invalid);
        }
        let baseline = intent.prepared.ok_or(Error::Invalid)?;
        let baseline: Vec<String> =
            serde_json::from_value(baseline["baseline"].clone()).map_err(|_| Error::Invalid)?;
        Ok(lists::catalogue(self.github, &token)
            .await?
            .into_iter()
            .filter(|l| {
                !baseline.contains(&l.id)
                    && Some(l.name.as_str()) == body["name"].as_str()
                    && l.description.as_deref().unwrap_or("")
                        == body["description"].as_str().unwrap_or("")
            })
            .map(|l| json!({"id":l.id,"name":l.name,"slug":l.slug,"description":l.description}))
            .collect())
    }
    pub async fn adopt_create(
        &self,
        user: i64,
        id: &str,
        remote_id: &str,
        clock: impl Fn() -> i64,
    ) -> Result<Intent> {
        let candidates = self.create_candidates(user, id).await?;
        let selected = candidates
            .into_iter()
            .find(|v| v["id"] == remote_id)
            .ok_or(Error::Invalid)?;
        self.authorize(user).await?;
        self.store
            .confirm_mutation(user, id, &selected, clock())
            .await?;
        self.store.apply_mutation(user, id, clock()).await
    }
}
impl std::fmt::Debug for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Intent")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}
impl Store {
    pub async fn reject_mutation(&self, user: i64, id: &str, code: &str, now: i64) -> Result<()> {
        if !["auth_failed", "rate_limited"].contains(&code) {
            return Err(Error::Invalid);
        }
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
 let intent=decode(query("SELECT * FROM inbox_gh_mutations WHERE user_id=? AND id=?").bind(user).bind(id).fetch_optional(&*c).await?.ok_or(Error::NotFound)?)?;
 if intent.state!="sending"{return Err(Error::MutationPending(id.into()))}
 if matches!(intent.request,Mutation::CreateList{..}) {query("DELETE FROM inbox_gh_folders WHERE id=? AND user_id=?").bind(intent.local_folder_id).bind(user).execute(&*c).await?;}
 query("UPDATE inbox_gh_mutations SET state='rejected',error_code=?,updated_at=? WHERE user_id=? AND id=? AND state='sending'").bind(code).bind(now).bind(user).bind(id).execute(&*c).await?;Ok(())}.await;
        match result {
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
}
