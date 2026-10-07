//! Fully paginated GitHub List reads for the star-sync workflow.
use crate::{Error, Result, github::Github};
use serde_json::{Value, json};
use std::collections::HashSet;
use yoink_db::{Executor, query};
pub struct ListSnapshot {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub items: Vec<(i64, String)>,
}
fn string(v: &Value, k: &str) -> Result<String> {
    v[k].as_str().map(str::to_owned).ok_or(Error::External)
}
fn next(v: &Value, seen: &mut HashSet<String>) -> Result<Option<String>> {
    if v["pageInfo"]["hasNextPage"] == false {
        return Ok(None);
    }
    if v["pageInfo"]["hasNextPage"] != true {
        return Err(Error::External);
    }
    let cursor = string(&v["pageInfo"], "endCursor")?;
    if cursor.is_empty() || !seen.insert(cursor.clone()) {
        return Err(Error::External);
    }
    Ok(Some(cursor))
}
pub async fn catalogue(github: &dyn Github, token: &str) -> Result<Vec<ListSnapshot>> {
    let viewer = github
        .graphql(token, "query { viewer { login } }", json!({}), false)
        .await?;
    let login = string(&viewer["viewer"], "login")?;
    let mut lists = Vec::new();
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let d=github.graphql(token,"query($login:String!,$after:String){user(login:$login){lists(first:100,after:$after){nodes{id name slug description} pageInfo{hasNextPage endCursor}}}}",json!({"login":login,"after":cursor}),false).await?;
        let p = &d["user"]["lists"];
        for v in p["nodes"].as_array().ok_or(Error::External)? {
            lists.push(ListSnapshot {
                id: string(v, "id")?,
                name: string(v, "name")?,
                slug: string(v, "slug")?,
                description: v["description"].as_str().map(str::to_owned),
                items: Vec::new(),
            });
        }
        cursor = next(p, &mut seen)?;
        if cursor.is_none() {
            break;
        }
    }
    Ok(lists)
}
pub async fn fetch(github: &dyn Github, token: &str) -> Result<Vec<ListSnapshot>> {
    let mut lists = catalogue(github, token).await?;
    for list in &mut lists {
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let d=github.graphql(token,"query($id:ID!,$after:String){node(id:$id){... on UserList{items(first:100,after:$after){nodes{... on Repository{id databaseId}} pageInfo{hasNextPage endCursor}}}}}",json!({"id":list.id,"after":cursor}),false).await?;
            let p = &d["node"]["items"];
            for v in p["nodes"].as_array().ok_or(Error::External)? {
                if let Some(id) = v["databaseId"].as_i64() {
                    list.items.push((id, string(v, "id")?));
                }
            }
            cursor = next(p, &mut seen)?;
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(lists)
}
/// Called in the star-snapshot transaction. Retains local folder order/pins,
/// local members and the legacy add-only synced membership behavior.
pub(crate) async fn apply<E: Executor>(
    c: &E,
    user: i64,
    lists: &[ListSnapshot],
    now: i64,
) -> Result<()> {
    let existing = query(
        "SELECT id,gh_list_id FROM inbox_gh_folders WHERE user_id=? AND gh_list_id IS NOT NULL",
    )
    .bind(user)
    .fetch_all(c)
    .await?;
    for list in lists {
        let found = query(
            "SELECT id FROM inbox_gh_folders WHERE user_id=? AND gh_list_id=? ORDER BY id LIMIT 1",
        )
        .bind(user)
        .bind(&list.id)
        .fetch_optional(c)
        .await?;
        let id = if let Some(row) = found {
            let id: i64 = row.try_get("id")?;
            query("UPDATE inbox_gh_folders SET name=?,gh_list_slug=? WHERE user_id=? AND id=?")
                .bind(&list.name)
                .bind(&list.slug)
                .bind(user)
                .bind(id)
                .execute(c)
                .await?;
            id
        } else {
            let mut slug = crate::normalize::slug(&list.name);
            if slug.is_empty() {
                slug = "list".into()
            }
            let base = slug.clone();
            let mut suffix = 1;
            while query("SELECT id FROM inbox_gh_folders WHERE user_id=? AND slug=?")
                .bind(user)
                .bind(&slug)
                .fetch_optional(c)
                .await?
                .is_some()
            {
                slug = format!("{base}-{suffix}");
                suffix += 1;
            }
            query("INSERT INTO inbox_gh_folders(user_id,name,slug,description,gh_list_id,gh_list_slug,created_at) VALUES(?,?,?,?,?,?,?) RETURNING id").bind(user).bind(&list.name).bind(slug).bind(&list.description).bind(&list.id).bind(&list.slug).bind(now).fetch_one(c).await?.try_get("id")?
        };
        for (repo, node) in &list.items {
            query("UPDATE inbox_gh_stars SET gh_node_id=? WHERE user_id=? AND gh_repo_id=?")
                .bind(node)
                .bind(user)
                .bind(*repo)
                .execute(c)
                .await?;
            query("INSERT INTO inbox_gh_folder_members(folder_id,gh_star_id,added_by,added_at) SELECT ?,id,'sync',? FROM inbox_gh_stars WHERE user_id=? AND gh_repo_id=? ON CONFLICT DO NOTHING").bind(id).bind(now).bind(user).bind(*repo).execute(c).await?;
        }
    }
    for row in existing {
        let remote: String = row.try_get("gh_list_id")?;
        if !lists.iter().any(|l| l.id == remote) {
            query("UPDATE inbox_gh_folders SET gh_list_id=NULL,gh_list_slug=NULL WHERE id=? AND user_id=?").bind(row.try_get::<i64,_>("id")?).bind(user).execute(c).await?;
        }
    }
    Ok(())
}

pub async fn create(
    github: &dyn Github,
    token: &str,
    name: &str,
    description: &str,
) -> Result<Value> {
    let d=github.graphql(token,"mutation($input:CreateUserListInput!){createUserList(input:$input){list{id name slug description}}}",json!({"input":{"name":name,"description":description,"isPrivate":false}}),true).await?;
    let list = d["createUserList"]
        .get("list")
        .cloned()
        .ok_or(Error::Uncertain)?;
    if list["id"].as_str().is_none_or(|s| s.is_empty())
        || !list["name"].is_string()
        || !list["slug"].is_string()
    {
        return Err(Error::Uncertain);
    }
    Ok(list)
}
pub async fn rename(github: &dyn Github, token: &str, id: &str, name: &str) -> Result<()> {
    let data = github
        .graphql(
            token,
            "mutation($input:UpdateUserListInput!){updateUserList(input:$input){list{id}}}",
            json!({"input":{"listId":id,"name":name}}),
            true,
        )
        .await?;
    if data["updateUserList"]["list"]["id"] != id {
        return Err(Error::Uncertain);
    }
    Ok(())
}
pub async fn delete(github: &dyn Github, token: &str, id: &str) -> Result<()> {
    let data = github
        .graphql(
            token,
            "mutation($input:DeleteUserListInput!){deleteUserList(input:$input){clientMutationId}}",
            json!({"input":{"listId":id}}),
            true,
        )
        .await?;
    if !data["deleteUserList"].is_object() {
        return Err(Error::Uncertain);
    }
    Ok(())
}
pub async fn membership(
    github: &dyn Github,
    token: &str,
    repo: &str,
    list: &str,
    add: bool,
) -> Result<()> {
    let mut ids = membership_ids(github, token, repo).await?;
    let exists = ids.iter().any(|i| i == list);
    if exists == add {
        return Ok(());
    }
    if add {
        ids.push(list.into())
    } else {
        ids.retain(|i| i != list)
    }
    replace_memberships(github, token, repo, &ids).await
}
pub async fn membership_ids(github: &dyn Github, token: &str, repo: &str) -> Result<Vec<String>> {
    let mut cursor = None;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    loop {
        let d=github.graphql(token,"query($id:ID!,$after:String){node(id:$id){... on Repository{lists(first:100,after:$after){nodes{id} pageInfo{hasNextPage endCursor}}}}}",json!({"id":repo,"after":cursor}),false).await?;
        let p = &d["node"]["lists"];
        for v in p["nodes"].as_array().ok_or(Error::External)? {
            ids.push(string(v, "id")?)
        }
        cursor = next(p, &mut seen)?;
        if cursor.is_none() {
            break;
        }
    }
    Ok(ids)
}
pub async fn replace_memberships(
    github: &dyn Github,
    token: &str,
    repo: &str,
    ids: &[String],
) -> Result<()> {
    let data=github.graphql(token,"mutation($input:UpdateUserListsForItemInput!){updateUserListsForItem(input:$input){item{id}}}",json!({"input":{"itemId":repo,"listIds":ids}}),true).await?;
    if data["updateUserListsForItem"]["item"]["id"] != repo {
        return Err(Error::Uncertain);
    }
    Ok(())
}
