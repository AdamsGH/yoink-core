use crate::{Error, Result, store::Store};
use serde_json::{Value, json};
use yoink_db::{Executor, query};
impl Store {
    pub async fn teams(&self, user: i64) -> Result<Value> {
        let ids=query("SELECT DISTINCT t.id FROM inbox_teams t LEFT JOIN inbox_team_members m ON m.team_id=t.id WHERE t.owner_user_id=? OR m.user_id=? ORDER BY t.id").bind(user).bind(user).fetch_all(&self.db).await?;
        let mut teams = Vec::new();
        for r in ids {
            teams.push(self.team(user, r.try_get("id")?).await?)
        }
        Ok(json!(teams))
    }
    pub async fn team(&self, user: i64, id: i64) -> Result<Value> {
        let r=query("SELECT t.* FROM inbox_teams t WHERE t.id=? AND (t.owner_user_id=? OR EXISTS(SELECT 1 FROM inbox_team_members m WHERE m.team_id=t.id AND m.user_id=?))").bind(id).bind(user).bind(user).fetch_optional(&self.db).await?.ok_or(Error::NotFound)?;
        let members=query("SELECT user_id,role,joined_at FROM inbox_team_members WHERE team_id=? ORDER BY user_id").bind(id).fetch_all(&self.db).await?;
        let mut all = Vec::new();
        for m in members {
            let ts: i64 = m.try_get("joined_at")?;
            all.push(json!({"user_id":m.try_get::<i64,_>("user_id")?,"role":m.try_get::<String,_>("role")?,"joined_at":chrono::DateTime::from_timestamp_micros(ts).map(|t|t.to_rfc3339())}));
        }
        let ts: i64 = r.try_get("created_at")?;
        Ok(
            json!({"id":id,"name":r.try_get::<String,_>("name")?,"slug":r.try_get::<String,_>("slug")?,"description":r.try_get::<Option<String>,_>("description")?,"owner_user_id":r.try_get::<i64,_>("owner_user_id")?,"created_at":chrono::DateTime::from_timestamp_micros(ts).map(|t|t.to_rfc3339()),"members":all}),
        )
    }
    pub async fn save_team(
        &self,
        user: i64,
        id: Option<i64>,
        body: &Value,
        now: i64,
    ) -> Result<Value> {
        let body = crate::validation::team(body)?;
        let name = body["name"]
            .as_str()
            .filter(|n| !n.is_empty() && n.chars().count() <= 128)
            .ok_or(Error::Invalid)?;
        let base = body["slug"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| crate::normalize::slug(name));
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result = async {
 let slug=crate::normalize::unique_slug(&*c,"inbox_teams","owner_user_id",user,if base.is_empty(){"team"}else{&base},id).await?;
 let row=if let Some(id)=id{query("UPDATE inbox_teams SET name=?,slug=?,description=? WHERE id=? AND owner_user_id=? RETURNING id").bind(name).bind(slug).bind(body["description"].as_str()).bind(id).bind(user).fetch_optional(&*c).await?.ok_or(Error::NotFound)?}else{query("INSERT INTO inbox_teams(name,slug,description,owner_user_id,created_at) VALUES(?,?,?,?,?) RETURNING id").bind(name).bind(slug).bind(body["description"].as_str()).bind(user).bind(now).fetch_one(&*c).await?};let id:i64=row.try_get("id")?;
 query("INSERT INTO inbox_team_members(team_id,user_id,role,joined_at) VALUES(?,?,'owner',?) ON CONFLICT DO NOTHING").bind(id).bind(user).bind(now).execute(&*c).await?;Ok(id)}.await;
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
        self.team(user, id).await
    }
    pub async fn delete_team(&self, user: i64, id: i64) -> Result<()> {
        if query("DELETE FROM inbox_teams WHERE id=? AND owner_user_id=?")
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
    /// Owner is invariant; only owner can promote an admin or remove one.
    pub async fn team_member(
        &self,
        user: i64,
        team: i64,
        target: i64,
        role: Option<&str>,
        now: i64,
    ) -> Result<Value> {
        if role.is_some_and(|s| !["admin", "member"].contains(&s)) {
            return Err(Error::Invalid);
        }
        let c = self.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result = async {
 let owner:i64=query("SELECT owner_user_id FROM inbox_teams WHERE id=?").bind(team).fetch_optional(&*c).await?.ok_or(Error::NotFound)?.try_get("owner_user_id")?;
 if target==owner{return Err(Error::Forbidden)}
 let caller=query("SELECT role FROM inbox_team_members WHERE team_id=? AND user_id=?").bind(team).bind(user).fetch_optional(&*c).await?.map(|r|r.try_get::<String,_>("role")).transpose()?;
 let prior=query("SELECT role FROM inbox_team_members WHERE team_id=? AND user_id=?").bind(team).bind(target).fetch_optional(&*c).await?.map(|r|r.try_get::<String,_>("role")).transpose()?;
 if user!=owner && (caller.as_deref()!=Some("admin") || role==Some("admin") || prior.as_deref()==Some("admin")) && !(role.is_none()&&user==target){return Err(Error::Forbidden)}
 if let Some(role)=role{query("INSERT INTO inbox_team_members(team_id,user_id,role,joined_at) VALUES(?,?,?,?) ON CONFLICT(team_id,user_id) DO UPDATE SET role=excluded.role").bind(team).bind(target).bind(role).bind(now).execute(&*c).await?;let row=query("SELECT joined_at FROM inbox_team_members WHERE team_id=? AND user_id=?").bind(team).bind(target).fetch_one(&*c).await?;let ts:i64=row.try_get("joined_at")?;Ok(json!({"user_id":target,"role":role,"joined_at":chrono::DateTime::from_timestamp_micros(ts).map(|t|t.to_rfc3339())}))}else{query("DELETE FROM inbox_team_members WHERE team_id=? AND user_id=?").bind(team).bind(target).execute(&*c).await?;Ok(Value::Null)}
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
