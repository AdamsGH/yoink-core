//! Native catalogue mutations. Parent/team references are checked in the same
//! write transaction and never authorize through an unscoped foreign key.
use crate::{Error, Result, normalize::category_key, store::Store};
use serde_json::{Value, json};
use yoink_db::{Executor, query};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Category,
    Folder,
    Rule,
}
impl Catalog {
    fn table(self) -> &'static str {
        match self {
            Self::Category => "inbox_categories",
            Self::Folder => "inbox_gh_folders",
            Self::Rule => "inbox_rules",
        }
    }
    fn owner(self) -> &'static str {
        if self == Self::Category {
            "owner_user_id"
        } else {
            "user_id"
        }
    }
}
impl Store {
    pub async fn catalogue(&self, user: i64, kind: Catalog) -> Result<Value> {
        if kind == Catalog::Category {
            return self.categories(user).await;
        }
        let sql = format!(
            "SELECT * FROM {} WHERE {}=? {} ORDER BY {}",
            kind.table(),
            kind.owner(),
            if kind == Catalog::Folder {
                "AND NOT EXISTS(SELECT 1 FROM inbox_gh_mutations m WHERE m.local_folder_id=inbox_gh_folders.id AND json_extract(m.request,'$.type')='create_list' AND m.state IN ('prepared','sending','unknown','confirmed'))"
            } else {
                ""
            },
            if kind == Catalog::Folder {
                "is_pinned DESC,sort_order,id"
            } else {
                "priority,id"
            }
        );
        let rows = query(&sql).bind(user).fetch_all(&self.db).await?;
        let mut all = Vec::new();
        for row in rows {
            let id: i64 = row.try_get("id")?;
            all.push(self.catalogue_one(user, kind, id).await?)
        }
        Ok(json!(all))
    }
    pub async fn catalogue_one(&self, user: i64, kind: Catalog, id: i64) -> Result<Value> {
        if kind == Catalog::Category {
            return self
                .categories(user)
                .await?
                .as_array()
                .and_then(|a| a.iter().find(|v| v["id"] == id))
                .cloned()
                .ok_or(Error::NotFound);
        }
        if kind==Catalog::Folder && query("SELECT id FROM inbox_gh_mutations WHERE local_folder_id=? AND user_id=? AND state IN ('prepared','sending','unknown','confirmed') AND json_extract(request,'$.type')='create_list'").bind(id).bind(user).fetch_optional(&self.db).await?.is_some(){return Err(Error::NotFound)}
        let sql = format!(
            "SELECT * FROM {} WHERE id=? AND {}=?",
            kind.table(),
            kind.owner()
        );
        let row = query(&sql)
            .bind(id)
            .bind(user)
            .fetch_optional(&self.db)
            .await?
            .ok_or(Error::NotFound)?;
        let mut v = serde_json::Map::new();
        v.insert("id".into(), json!(id));
        v.insert("user_id".into(), json!(user));
        v.insert("name".into(), json!(row.try_get::<String, _>("name")?));
        for key in if kind == Catalog::Folder {
            vec!["created_at"]
        } else {
            vec!["created_at", "modified_at"]
        } {
            let ts: i64 = row.try_get(key)?;
            v.insert(
                key.into(),
                json!(chrono::DateTime::from_timestamp_micros(ts).map(|t| t.to_rfc3339())),
            );
        }
        if kind == Catalog::Folder {
            v.insert("slug".into(), json!(row.try_get::<String, _>("slug")?));
            for key in ["description", "icon", "gh_list_id", "gh_list_slug"] {
                v.insert(key.into(), json!(row.try_get::<Option<String>, _>(key)?));
            }
            v.insert(
                "parent_id".into(),
                json!(row.try_get::<Option<i64>, _>("parent_id")?),
            );
            v.insert(
                "is_pinned".into(),
                json!(row.try_get::<bool, _>("is_pinned")?),
            );
            v.insert(
                "sort_order".into(),
                json!(row.try_get::<i64, _>("sort_order")?),
            );
            let n:i64=query("SELECT count(*) AS n FROM inbox_gh_folder_members m JOIN inbox_gh_stars s ON s.id=m.gh_star_id WHERE m.folder_id=? AND s.user_id=?").bind(id).bind(user).fetch_one(&self.db).await?.try_get("n")?;
            v.insert("star_count".into(), json!(n));
        } else {
            v.insert("enabled".into(), json!(row.try_get::<bool, _>("enabled")?));
            v.insert("priority".into(), json!(row.try_get::<i64, _>("priority")?));
            v.insert(
                "trigger".into(),
                json!(row.try_get::<String, _>("trigger")?),
            );
            for key in ["conditions", "actions"] {
                v.insert(
                    key.into(),
                    row.try_get::<Option<Value>, _>(key)?.unwrap_or(Value::Null),
                );
            }
        }
        Ok(Value::Object(v))
    }
    pub async fn save_catalogue(
        &self,
        user: i64,
        kind: Catalog,
        id: Option<i64>,
        patch: &Value,
        now: i64,
    ) -> Result<Value> {
        if patch.get("gh_list_id").is_some() || patch.get("gh_list_slug").is_some() {
            return Err(Error::Invalid);
        }
        if !patch.is_object() {
            return Err(Error::Invalid);
        }
        let patch =
            crate::validation::catalogue(patch, kind, id.is_none() && kind != Catalog::Folder)?;
        let mut body = if let Some(id) = id {
            self.catalogue_one(user, kind, id).await?
        } else {
            json!({})
        };
        for (k, v) in patch.as_object().unwrap() {
            body[k] = v.clone();
        }
        let name = body["name"]
            .as_str()
            .filter(|n| !n.is_empty() && n.chars().count() <= 128)
            .ok_or(Error::Invalid)?
            .to_owned();
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result = async {
 if let Some(id)=id {let sql=format!("SELECT id FROM {} WHERE id=? AND {}=?",kind.table(),kind.owner());query(&sql).bind(id).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;}
 let parent=body["parent_id"].as_i64();
 if let Some(parent)=parent {if kind==Catalog::Rule || Some(parent)==id{return Err(Error::Invalid)}let sql=format!("SELECT id FROM {} WHERE id=? AND {}=?",kind.table(),kind.owner());query(&sql).bind(parent).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;
 // Reject indirect cycles using the module-owned parent chain.
 let mut current=Some(parent);let mut visited=std::collections::HashSet::new();while let Some(p)=current{if Some(p)==id||!visited.insert(p){return Err(Error::Invalid)}let sql=format!("SELECT parent_id FROM {} WHERE id=? AND {}=?",kind.table(),kind.owner());current=query(&sql).bind(p).bind(user).fetch_one(&*c).await?.try_get("parent_id")?;}}
 let team=body["shared_with_team_id"].as_i64();if let Some(team)=team{if kind!=Catalog::Category{return Err(Error::Invalid)}query("SELECT team_id FROM inbox_team_members WHERE team_id=? AND user_id=?").bind(team).bind(user).fetch_optional(&*c).await?.ok_or(Error::Forbidden)?;}
 let mut candidate=body["slug"].as_str().filter(|s|!s.is_empty()).map(str::to_owned).unwrap_or_else(||crate::normalize::slug(&name));if candidate.is_empty(){candidate=if kind==Catalog::Folder{"folder"}else{"category"}.into()}
 candidate=candidate.chars().take(64).collect();
 if kind!=Catalog::Rule {candidate=crate::normalize::unique_slug(&*c,kind.table(),kind.owner(),user,&candidate,id).await?;}
 let icon=body["icon"].as_str();let description=body["description"].as_str();
 let row=match kind {
 Catalog::Category=>{let key=category_key(&name);let color=body["color"].as_str();if let Some(id)=id{query("UPDATE inbox_categories SET name=?,normalized_name=?,slug=?,icon=?,color=?,description=?,parent_id=?,shared_with_team_id=? WHERE id=? AND owner_user_id=? RETURNING id").bind(&name).bind(key).bind(candidate).bind(icon).bind(color).bind(description).bind(parent).bind(team).bind(id).bind(user).fetch_one(&*c).await?}else{query("INSERT INTO inbox_categories(owner_user_id,name,normalized_name,slug,icon,color,description,parent_id,shared_with_team_id,kind,created_at) VALUES(?,?,?,?,?,?,?,?,?,'user',?) RETURNING id").bind(user).bind(&name).bind(key).bind(candidate).bind(icon).bind(color).bind(description).bind(parent).bind(team).bind(now).fetch_one(&*c).await?}},
 Catalog::Folder=>{
 let pinned=body["is_pinned"].as_bool().unwrap_or(false);let order=body["sort_order"].as_i64().unwrap_or(0);if let Some(id)=id {query("UPDATE inbox_gh_folders SET name=?,slug=?,description=?,icon=?,parent_id=?,is_pinned=?,sort_order=? WHERE id=? AND user_id=? RETURNING id").bind(&name).bind(candidate).bind(description).bind(icon).bind(parent).bind(pinned).bind(order).bind(id).bind(user).fetch_one(&*c).await?}else{query("INSERT INTO inbox_gh_folders(user_id,name,slug,description,icon,parent_id,is_pinned,sort_order,created_at) VALUES(?,?,?,?,?,?,?,?,?) RETURNING id").bind(user).bind(&name).bind(candidate).bind(description).bind(icon).bind(parent).bind(pinned).bind(order).bind(now).fetch_one(&*c).await?}},
 Catalog::Rule=>{let trigger=body["trigger"].as_str().filter(|s|s.chars().count()<=24).ok_or(Error::Invalid)?;let enabled=body["enabled"].as_bool().unwrap_or(true);let priority=body["priority"].as_i64().unwrap_or(100);let conditions=body.get("conditions").filter(|v|!v.is_null());let actions=body.get("actions").filter(|v|!v.is_null());if conditions.is_some_and(|v|!v.is_array())||actions.is_some_and(|v|!v.is_array()){return Err(Error::Invalid)}
 if let Some(id)=id{query("UPDATE inbox_rules SET name=?,enabled=?,priority=?,trigger=?,conditions=?,actions=?,modified_at=? WHERE id=? AND user_id=? RETURNING id").bind(&name).bind(enabled).bind(priority).bind(trigger).bind(conditions).bind(actions).bind(now).bind(id).bind(user).fetch_one(&*c).await?}else{query("INSERT INTO inbox_rules(user_id,name,enabled,priority,trigger,conditions,actions,created_at,modified_at) VALUES(?,?,?,?,?,?,?,?,?) RETURNING id").bind(user).bind(&name).bind(enabled).bind(priority).bind(trigger).bind(conditions).bind(actions).bind(now).bind(now).fetch_one(&*c).await?}}
 };Ok(row.try_get::<i64,_>("id")?)
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
        self.catalogue_one(user, kind, id).await
    }
    pub async fn delete_catalogue(&self, user: i64, kind: Catalog, id: i64) -> Result<()> {
        let sql = format!(
            "DELETE FROM {} WHERE id=? AND {}=?",
            kind.table(),
            kind.owner()
        );
        if query(&sql)
            .bind(id)
            .bind(user)
            .execute(&self.db)
            .await?
            .rows_affected()
            == 0
        {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    pub async fn membership_confirmed(
        &self,
        user: i64,
        folder: i64,
        star: i64,
        add: bool,
        now: i64,
    ) -> Result<()> {
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result = async {
 let row=query("SELECT gh_list_id FROM inbox_gh_folders WHERE id=? AND user_id=?").bind(folder).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;let _ = row.try_get::<Option<String>,_>("gh_list_id")?;query("SELECT id FROM inbox_gh_stars WHERE id=? AND user_id=?").bind(star).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?;
 if add{query("INSERT INTO inbox_gh_folder_members(folder_id,gh_star_id,added_by,added_at) VALUES(?,?,'user',?) ON CONFLICT DO NOTHING").bind(folder).bind(star).bind(now).execute(&*c).await?;}else{query("DELETE FROM inbox_gh_folder_members WHERE folder_id=? AND gh_star_id=?").bind(folder).bind(star).execute(&*c).await?;}Ok(())}.await;
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
