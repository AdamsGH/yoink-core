#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, Notify},
};
use yoink_db::{Database, Executor, query};
use yoink_inbox::{
    Error,
    contracts::{Authorization, Call, Insight},
    github::GithubClient,
    store::Store,
};
pub struct Auth;
impl Authorization for Auth {
    fn require<'a>(&'a self, user: i64, _feature: &'a str) -> Call<'a, ()> {
        Box::pin(async move {
            if user == 3 {
                Err(Error::Forbidden)
            } else {
                Ok(())
            }
        })
    }
    fn require_admin(&self, _user: i64) -> Call<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
pub struct Keys;
impl Insight for Keys {
    fn github_token(&self, _user: i64, _write: bool) -> Call<'_, Option<String>> {
        Box::pin(async { Ok(Some("fixture-secret".into())) })
    }
    fn complete<'a>(&'a self, _user: i64, _prompt: &'a str) -> Call<'a, String> {
        Box::pin(async { Ok("{}".into()) })
    }
    fn language(&self, _user: i64) -> Call<'_, String> {
        Box::pin(async { Ok("en".into()) })
    }
}
pub async fn db() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let d = Database::open(&dir.path().join("db"), true, Duration::from_secs(10))
        .await
        .unwrap();
    d.batch(include_str!(
        "../../../yoink-storage/migrations-turso/0002_application.sql"
    ))
    .await
    .unwrap();
    d.batch(include_str!("../../sql/durable_jobs.sql"))
        .await
        .unwrap();
    d.batch(include_str!("../../sql/github_mutations.sql"))
        .await
        .unwrap();
    d.batch(include_str!("../../sql/notification_outbox.sql"))
        .await
        .unwrap();
    for id in 1..=3 {
        query("INSERT INTO users(id,created_at,updated_at) VALUES(?,1,1)")
            .bind(id)
            .execute(&d)
            .await
            .unwrap();
    }
    (dir, Store::new(d))
}
#[derive(Clone)]
pub struct RemoteList {
    pub name: String,
    pub description: String,
    pub slug: String,
}
#[derive(Default)]
pub struct Remote {
    pub lists: BTreeMap<String, RemoteList>,
    pub memberships: BTreeSet<String>,
    pub next: i64,
    pub writes: Vec<String>,
    pub requests: Vec<Value>,
    pub drop_after: Option<String>,
    pub pause_after: Option<String>,
    pub rejected: Option<String>,
    pub starred: bool,
    pub pages: BTreeMap<String, String>,
    pub raw_requests: Vec<String>,
    pub github_unavailable: bool,
}
pub struct Fixture {
    endpoint: url::Url,
    pub state: Arc<Mutex<Remote>>,
    pub client: GithubClient,
    pub reached: Arc<Notify>,
    pub release: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn page(nodes: Vec<Value>, after: Option<&str>) -> Value {
    let n = after.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
    json!({"nodes":nodes.iter().skip(n).take(1).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":n+1<nodes.len(),"endCursor":(n+1).to_string()}})
}
impl Fixture {
    pub fn endpoint(&self) -> url::Url {
        self.endpoint.clone()
    }
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let state = Arc::new(Mutex::new(Remote::default()));
        let reached = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let s = state.clone();
        let r = reached.clone();
        let wake = release.clone();
        let task = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let s = s.clone();
                let r = r.clone();
                let wake = wake.clone();
                tokio::spawn(async move {
                    let mut socket = socket;
                    let mut bytes = Vec::new();
                    let mut buf = [0; 4096];
                    let (header_end, len) = loop {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let header = String::from_utf8_lossy(&bytes[..end]);
                            let len = header
                                .lines()
                                .find_map(|l| {
                                    l.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|s| s.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            break (end + 4, len);
                        }
                    };
                    while bytes.len() < header_end + len {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    let header = String::from_utf8_lossy(&bytes[..header_end]);
                    let mut state = s.lock().await;
                    let mut op = "read".to_owned();
                    let mut status = "200 OK";
                    let body = if header.starts_with("POST /graphql") {
                        let v: Value =
                            serde_json::from_slice(&bytes[header_end..header_end + len]).unwrap();
                        state.requests.push(v.clone());
                        let q = v["query"].as_str().unwrap();
                        let vars = &v["variables"];
                        let input = &vars["input"];
                        let after = vars["after"].as_str();
                        let reject = state.rejected.as_deref().is_some_and(|op| match op {
                            "create" => q.contains("createUserList"),
                            "edit" => q.contains("updateUserList(input"),
                            "delete" => q.contains("deleteUserList"),
                            "membership" => q.contains("updateUserListsForItem"),
                            _ => false,
                        });
                        if reject {
                            state.rejected = None;
                            status = "403 Forbidden";
                        }
                        let data = if reject {
                            json!(null)
                        } else if q.contains("createUserList") {
                            op = "create".into();
                            state.next += 1;
                            let id = format!("new{}", state.next);
                            let name = input["name"].as_str().unwrap().to_owned();
                            let description = input["description"].as_str().unwrap().to_owned();
                            let slug = format!("slug-{}", state.next);
                            state.lists.insert(
                                id.clone(),
                                RemoteList {
                                    name: name.clone(),
                                    description: description.clone(),
                                    slug,
                                },
                            );
                            json!({"createUserList":{"list":{"id":id,"name":name,"slug":format!("slug-{}",state.next),"description":description}}})
                        } else if q.contains("updateUserList(input") {
                            op = "edit".into();
                            let id = input["listId"].as_str().unwrap();
                            if let Some(l) = state.lists.get_mut(id) {
                                l.name = input["name"].as_str().unwrap().into();
                            }
                            json!({"updateUserList":{"list":{"id":id}}})
                        } else if q.contains("deleteUserList") {
                            op = "delete".into();
                            state.lists.remove(input["listId"].as_str().unwrap());
                            json!({"deleteUserList":{"clientMutationId":null}})
                        } else if q.contains("updateUserListsForItem") {
                            op = "membership".into();
                            state.memberships = input["listIds"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|v| v.as_str().unwrap().to_owned())
                                .collect();
                            json!({"updateUserListsForItem":{"item":{"id":input["itemId"]}}})
                        } else if q.contains("viewer") {
                            json!({"viewer":{"login":"fixture"}})
                        } else if q.contains("user(login") {
                            let nodes=state.lists.iter().map(|(id,l)|json!({"id":id,"name":l.name,"slug":l.slug,"description":l.description,"isPrivate":false,"items":{"totalCount":0}})).collect();
                            json!({"user":{"lists":page(nodes,after)}})
                        } else if q.contains("items(first") {
                            let id = vars["id"].as_str().unwrap();
                            let nodes = if state.memberships.contains(id) {
                                vec![json!({"id":"R1","databaseId":10})]
                            } else {
                                vec![]
                            };
                            json!({"node":{"items":page(nodes,after)}})
                        } else if q.contains("lists(first") {
                            let nodes = state
                                .memberships
                                .iter()
                                .map(|id| json!({"id":id}))
                                .collect();
                            json!({"node":{"lists":page(nodes,after)}})
                        } else {
                            let id = vars["id"].as_str().unwrap();
                            json!({"node":state.lists.get(id).map(|l|json!({"id":id,"name":l.name}))})
                        };
                        if op != "read" {
                            state.writes.push(op.clone());
                        }
                        json!({"data":data}).to_string()
                    } else if header.starts_with("GET /user/starred/") {
                        status = if state.starred {
                            "204 No Content"
                        } else {
                            "404 Not Found"
                        };
                        String::new()
                    } else if header.starts_with("PUT /user/starred/")
                        || header.starts_with("DELETE /user/starred/")
                    {
                        op = "star".into();
                        state.starred = header.starts_with("PUT");
                        state.writes.push(op.clone());
                        status = "204 No Content";
                        String::new()
                    } else if header.starts_with("GET /content?") {
                        assert!(!header.to_lowercase().contains("authorization:"));
                        let path = header.split_whitespace().nth(1).unwrap();
                        let url = url::Url::parse(&format!("http://fixture{path}")).unwrap();
                        let target = url
                            .query_pairs()
                            .find(|(k, _)| k == "target")
                            .unwrap()
                            .1
                            .into_owned();
                        state.raw_requests.push(target.clone());
                        match state.pages.get(&target) {
                            Some(body) => body.clone(),
                            None => {
                                status = "404 Not Found";
                                String::new()
                            }
                        }
                    } else if header.starts_with("GET /repos/") && state.github_unavailable {
                        status = "403 Forbidden";
                        String::new()
                    } else if header.starts_with("GET /repos/a/b/readme") {
                        json!({"content":"UmVhZG1l"}).to_string()
                    } else if header.starts_with("GET /repos/a/b") {
                        json!({"id":10,"full_name":"a/b","description":"Repo seed","owner":{"login":"a","avatar_url":"https://example.com/avatar"},"html_url":"https://github.com/a/b","stargazers_count":7,"language":"Rust"}).to_string()
                    } else {
                        status = "404 Not Found";
                        String::new()
                    };
                    let disconnect = state.drop_after.as_deref() == Some(&op);
                    if disconnect {
                        state.drop_after = None;
                    }
                    let pause = state.pause_after.as_deref() == Some(&op);
                    if pause {
                        state.pause_after = None;
                    }
                    drop(state);
                    if pause {
                        r.notify_one();
                        wake.notified().await;
                    }
                    if disconnect {
                        return;
                    }
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        Self {
            state,
            endpoint: base.clone(),
            client: GithubClient::new(Duration::from_secs(5), None)
                .unwrap()
                .with_endpoint(base)
                .unwrap(),
            reached,
            release,
            task,
        }
    }
    pub async fn list(&self, id: &str, name: &str) {
        self.state.lock().await.lists.insert(
            id.into(),
            RemoteList {
                name: name.into(),
                slug: id.into(),
                description: String::new(),
            },
        );
    }
}
pub async fn seed_star(store: &Store) -> i64 {
    query("INSERT INTO inbox_gh_stars(user_id,gh_repo_id,gh_node_id,full_name,owner_login,html_url,last_synced_at,ai_summary,ai_labels) VALUES(1,10,'R1','a/b','a','https://github.com/a/b',1,'keep summary','[\"keep label\"]') RETURNING id").fetch_one(&store.db).await.unwrap().try_get("id").unwrap()
}
pub async fn seed_folder(store: &Store, remote: &str) -> i64 {
    query("INSERT INTO inbox_gh_folders(user_id,name,slug,gh_list_id,is_pinned,sort_order,created_at) VALUES(1,'Old',?,?,1,42,1) RETURNING id").bind(remote).bind(remote).fetch_one(&store.db).await.unwrap().try_get("id").unwrap()
}
