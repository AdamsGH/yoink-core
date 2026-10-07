//! Async GitHub REST/GraphQL adapter. No automatic mutation retries; error bodies
//! and reqwest errors are intentionally discarded to prevent secret exposure.
use crate::{Error, Result, contracts::Call};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};
use url::Url;
#[derive(Clone)]
pub struct GithubClient {
    client: Client,
    base: Url,
    raw_base: Url,
}
#[derive(Clone, Debug)]
pub struct StarSnapshot {
    pub rows: Vec<Value>,
    pub etag: Option<String>,
    pub can_unstar: bool,
    pub not_modified: bool,
}
pub trait Github: Send + Sync {
    fn stars<'a>(&'a self, token: &'a str, etag: Option<&'a str>) -> Call<'a, StarSnapshot>;
    fn star<'a>(
        &'a self,
        token: &'a str,
        owner: &'a str,
        repo: &'a str,
        star: bool,
    ) -> Call<'a, ()>;
    /// Fixed GitHub raw origin, never a user-selected origin or provider URL.
    fn raw_file<'a>(&'a self, _token: Option<&'a str>, _path: &'a str) -> Call<'a, String> {
        Box::pin(async { Err(Error::External) })
    }
    fn get_json<'a>(&'a self, token: Option<&'a str>, path: &'a str) -> Call<'a, Value>;
    fn is_starred<'a>(&'a self, token: &'a str, owner: &'a str, repo: &'a str) -> Call<'a, bool>;
    fn graphql<'a>(
        &'a self,
        token: &'a str,
        query: &'a str,
        variables: Value,
        mutation: bool,
    ) -> Call<'a, Value>;
}
impl GithubClient {
    pub fn new(timeout: Duration, proxy: Option<&str>) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid);
        }
        let mut builder = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("yoink-inbox/0.1");
        if let Some(proxy) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| Error::Invalid)?)
        }
        Ok(Self {
            client: builder.build().map_err(|_| Error::External)?,
            base: Url::parse("https://api.github.com/").unwrap(),
            raw_base: Url::parse("https://raw.githubusercontent.com/").unwrap(),
        })
    }
    /// Injectable endpoint for disposable fixtures/operator-controlled gateways;
    /// never accept this value from a user request. Redirects remain disabled.
    pub fn with_endpoint(mut self, base: Url) -> Result<Self> {
        if !["http", "https"].contains(&base.scheme())
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(Error::Invalid);
        }
        self.raw_base = base.join("raw/").map_err(|_| Error::Invalid)?;
        self.base = base;
        Ok(self)
    }
    async fn gql(
        &self,
        token: &str,
        query: &str,
        variables: Value,
        mutation: bool,
    ) -> Result<Value> {
        let response = self
            .client
            .post(self.base.join("graphql").map_err(|_| Error::Invalid)?)
            .bearer_auth(token)
            .json(&json!({"query":query,"variables":variables}))
            .send()
            .await
            .map_err(|_| {
                if mutation {
                    Error::Uncertain
                } else {
                    Error::External
                }
            })?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || (response.status() == StatusCode::FORBIDDEN
                && response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .is_some_and(|v| v == "0"))
        {
            return Err(Error::RateLimited);
        }
        if !response.status().is_success() {
            return Err(if mutation && response.status().is_server_error() {
                Error::Uncertain
            } else if [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN].contains(&response.status())
            {
                Error::Forbidden
            } else {
                Error::External
            });
        }
        let payload: Value = response.json().await.map_err(|_| {
            if mutation {
                Error::Uncertain
            } else {
                Error::External
            }
        })?;
        if payload.get("errors").is_some() {
            return Err(if mutation {
                Error::Uncertain
            } else {
                Error::External
            });
        }
        payload.get("data").cloned().ok_or(if mutation {
            Error::Uncertain
        } else {
            Error::External
        })
    }
    pub async fn viewer(&self, token: &str) -> Result<String> {
        let d = self
            .gql(token, "query { viewer { login } }", json!({}), false)
            .await?;
        str_at(&d["viewer"], "login")
    }
    /// All lists, not just the first 100.
    pub async fn lists(&self, token: &str, login: &str) -> Result<Vec<Value>> {
        let q = "query($login:String!,$after:String){user(login:$login){lists(first:100,after:$after){nodes{id name slug description isPrivate items{totalCount}} pageInfo{hasNextPage endCursor}}}}";
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        loop {
            let d = self
                .gql(token, q, json!({"login":login,"after":cursor}), false)
                .await?;
            let page = &d["user"]["lists"];
            all.extend(
                page["nodes"]
                    .as_array()
                    .ok_or(Error::External)?
                    .iter()
                    .cloned(),
            );
            if !advance(page, &mut cursor, &mut seen)? {
                break;
            }
        }
        Ok(all)
    }
    pub async fn list_items(&self, token: &str, list: &str) -> Result<Vec<(i64, String)>> {
        let q = "query($id:ID!,$after:String){node(id:$id){... on UserList{items(first:100,after:$after){nodes{... on Repository{id databaseId}} pageInfo{hasNextPage endCursor}}}}}";
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        loop {
            let d = self
                .gql(token, q, json!({"id":list,"after":cursor}), false)
                .await?;
            let page = &d["node"]["items"];
            for row in page["nodes"].as_array().ok_or(Error::External)? {
                if let (Some(id), Some(node)) = (row["databaseId"].as_i64(), row["id"].as_str()) {
                    all.push((id, node.into()))
                }
            }
            if !advance(page, &mut cursor, &mut seen)? {
                break;
            }
        }
        Ok(all)
    }
    pub async fn create_list(&self, token: &str, name: &str, description: &str) -> Result<Value> {
        let d=self.gql(token,"mutation($input:CreateUserListInput!){createUserList(input:$input){list{id name slug description isPrivate}}}",json!({"input":{"name":name,"description":description,"isPrivate":false}}),true).await?;
        d["createUserList"]
            .get("list")
            .cloned()
            .ok_or(Error::Uncertain)
    }
    pub async fn rename_list(&self, token: &str, id: &str, name: &str) -> Result<()> {
        self.gql(
            token,
            "mutation($input:UpdateUserListInput!){updateUserList(input:$input){list{id}}}",
            json!({"input":{"listId":id,"name":name}}),
            true,
        )
        .await?;
        Ok(())
    }
    pub async fn delete_list(&self, token: &str, id: &str) -> Result<()> {
        self.gql(
            token,
            "mutation($input:DeleteUserListInput!){deleteUserList(input:$input){clientMutationId}}",
            json!({"input":{"listId":id}}),
            true,
        )
        .await?;
        Ok(())
    }
    /// Membership is a full replacement remotely. Read every page, preserve other lists.
    /// Coordinator must serialize mutations per user/repository; GitHub has no CAS.
    pub async fn membership(&self, token: &str, repo: &str, list: &str, add: bool) -> Result<()> {
        let q = "query($id:ID!,$after:String){node(id:$id){... on Repository{lists(first:100,after:$after){nodes{id} pageInfo{hasNextPage endCursor}}}}}";
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        loop {
            let d = self
                .gql(token, q, json!({"id":repo,"after":cursor}), false)
                .await?;
            let page = &d["node"]["lists"];
            for n in page["nodes"].as_array().ok_or(Error::External)? {
                ids.push(str_at(n, "id")?)
            }
            if !advance(page, &mut cursor, &mut seen)? {
                break;
            }
        }
        let exists = ids.iter().any(|i| i == list);
        if exists == add {
            return Ok(());
        }
        if add {
            ids.push(list.into())
        } else {
            ids.retain(|i| i != list)
        }
        self.gql(token,"mutation($input:UpdateUserListsForItemInput!){updateUserListsForItem(input:$input){item{id}}}",json!({"input":{"itemId":repo,"listIds":ids}}),true).await?;
        Ok(())
    }
}
impl Github for GithubClient {
    fn stars<'a>(&'a self, token: &'a str, etag: Option<&'a str>) -> Call<'a, StarSnapshot> {
        Box::pin(async move {
            let mut next = Some(
                self.base
                    .join("user/starred?per_page=100&sort=created&direction=desc")
                    .map_err(|_| Error::Invalid)?,
            );
            let mut visited = HashSet::new();
            let mut rows = Vec::new();
            let mut first = true;
            let mut new_etag = None;
            let mut can_unstar = false;
            while let Some(url) = next.take() {
                if url.origin() != self.base.origin()
                    || !url.path().starts_with("/user/starred")
                    || !visited.insert(url.to_string())
                {
                    return Err(Error::External);
                }
                let mut request = self
                    .client
                    .get(url)
                    .bearer_auth(token)
                    .header("Accept", "application/vnd.github.star+json")
                    .header("X-GitHub-Api-Version", "2022-11-28");
                if first && let Some(etag) = etag {
                    request = request.header("If-None-Match", etag);
                }
                let response = request.send().await.map_err(|_| Error::External)?;
                if first && response.status() == StatusCode::NOT_MODIFIED {
                    return Ok(StarSnapshot {
                        rows,
                        etag: etag.map(str::to_owned),
                        can_unstar: false,
                        not_modified: true,
                    });
                }
                if !response.status().is_success() {
                    return Err(
                        if [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN]
                            .contains(&response.status())
                        {
                            Error::Forbidden
                        } else {
                            Error::External
                        },
                    );
                }
                if first {
                    new_etag = response
                        .headers()
                        .get("etag")
                        .and_then(|h| h.to_str().ok())
                        .map(str::to_owned);
                    can_unstar = response
                        .headers()
                        .get("x-oauth-scopes")
                        .and_then(|h| h.to_str().ok())
                        .is_some_and(|s| {
                            s.split(',')
                                .any(|p| matches!(p.trim(), "repo" | "public_repo"))
                        });
                    first = false;
                }
                next = response
                    .headers()
                    .get("link")
                    .and_then(|h| h.to_str().ok())
                    .map(next_link)
                    .transpose()?
                    .flatten();
                let page: Value = response.json().await.map_err(|_| Error::External)?;
                for row in page.as_array().ok_or(Error::External)? {
                    if row["repo"]["id"].as_i64().is_none() {
                        return Err(Error::External);
                    }
                    rows.push(row.clone());
                }
            }
            Ok(StarSnapshot {
                rows,
                etag: new_etag,
                can_unstar,
                not_modified: false,
            })
        })
    }
    fn star<'a>(
        &'a self,
        token: &'a str,
        owner: &'a str,
        repo: &'a str,
        star: bool,
    ) -> Call<'a, ()> {
        Box::pin(async move {
            if [owner, repo].iter().any(|s| {
                s.is_empty()
                    || !s
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            }) {
                return Err(Error::Invalid);
            }
            let url = self
                .base
                .join(&format!("user/starred/{owner}/{repo}"))
                .map_err(|_| Error::Invalid)?;
            let r = self
                .client
                .request(if star { Method::PUT } else { Method::DELETE }, url)
                .bearer_auth(token)
                .header("Accept", "application/vnd.github+json")
                .header("Content-Length", "0")
                .send()
                .await
                .map_err(|_| Error::Uncertain)?;
            match r.status() {
                StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED => Ok(()),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Forbidden),
                s if s.is_server_error() => Err(Error::Uncertain),
                _ => Err(Error::External),
            }
        })
    }

    fn raw_file<'a>(&'a self, token: Option<&'a str>, path: &'a str) -> Call<'a, String> {
        Box::pin(async move {
            if path.starts_with('/')
                || path.contains("://")
                || path.contains("..")
                || path.contains(['?', '#'])
            {
                return Err(Error::Invalid);
            }
            let u = self.raw_base.join(path).map_err(|_| Error::Invalid)?;
            if u.origin() != self.raw_base.origin() {
                return Err(Error::Invalid);
            }
            let mut req = self.client.get(u);
            if let Some(token) = token {
                req = req.bearer_auth(token)
            }
            let mut r = req.send().await.map_err(|_| Error::External)?;
            if !r.status().is_success() {
                return Err(Error::External);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = r.chunk().await.map_err(|_| Error::External)? {
                if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                    return Err(Error::External);
                }
                bytes.extend_from_slice(&chunk)
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })
    }
    fn get_json<'a>(&'a self, token: Option<&'a str>, path: &'a str) -> Call<'a, Value> {
        Box::pin(async move {
            if path.starts_with(['/', '?'])
                || path.contains("://")
                || path.contains("..")
                || path.contains('#')
            {
                return Err(Error::Invalid);
            }
            let u = self.base.join(path).map_err(|_| Error::Invalid)?;
            if u.origin() != self.base.origin() {
                return Err(Error::Invalid);
            }
            let mut req = self
                .client
                .get(u)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Some(t) = token {
                req = req.bearer_auth(t)
            }
            let mut r = req.send().await.map_err(|_| Error::External)?;
            if !r.status().is_success() {
                return Err(match r.status() {
                    StatusCode::NOT_FOUND => Error::NotFound,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Error::Forbidden,
                    StatusCode::TOO_MANY_REQUESTS => Error::RateLimited,
                    _ => Error::External,
                });
            }
            let mut body = Vec::new();
            while let Some(chunk) = r.chunk().await.map_err(|_| Error::External)? {
                if body.len() + chunk.len() > 2 * 1024 * 1024 {
                    return Err(Error::External);
                }
                body.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&body).map_err(|_| Error::External)
        })
    }
    fn is_starred<'a>(&'a self, token: &'a str, owner: &'a str, repo: &'a str) -> Call<'a, bool> {
        Box::pin(async move {
            if [owner, repo].iter().any(|s| {
                s.is_empty()
                    || !s
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            }) {
                return Err(Error::Invalid);
            }
            let r = self
                .client
                .get(
                    self.base
                        .join(&format!("user/starred/{owner}/{repo}"))
                        .map_err(|_| Error::Invalid)?,
                )
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| Error::External)?;
            match r.status() {
                StatusCode::NO_CONTENT => Ok(true),
                StatusCode::NOT_FOUND => Ok(false),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Forbidden),
                _ => Err(Error::External),
            }
        })
    }
    fn graphql<'a>(
        &'a self,
        token: &'a str,
        query: &'a str,
        variables: Value,
        mutation: bool,
    ) -> Call<'a, Value> {
        Box::pin(self.gql(token, query, variables, mutation))
    }
}
fn str_at(v: &Value, key: &str) -> Result<String> {
    v[key].as_str().map(str::to_owned).ok_or(Error::External)
}
fn advance(page: &Value, cursor: &mut Option<String>, seen: &mut HashSet<String>) -> Result<bool> {
    match page["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or(Error::External)?
    {
        false => Ok(false),
        true => {
            let next = str_at(&page["pageInfo"], "endCursor")?;
            if next.is_empty() || !seen.insert(next.clone()) {
                return Err(Error::External);
            }
            *cursor = Some(next);
            Ok(true)
        }
    }
}
fn next_link(header: &str) -> Result<Option<Url>> {
    for part in header.split(',') {
        if part.split(';').skip(1).any(|p| p.trim() == "rel=\"next\"") {
            let raw = part.split(';').next().ok_or(Error::External)?.trim();
            return Url::parse(
                raw.strip_prefix('<')
                    .and_then(|s| s.strip_suffix('>'))
                    .ok_or(Error::External)?,
            )
            .map(Some)
            .map_err(|_| Error::External);
        }
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links() {
        assert_eq!(next_link("<https://api.github.com/user/starred?page=2>; rel=\"next\", <https://api.github.com/x>; rel=\"last\"").unwrap().unwrap().query(),Some("page=2"));
        assert!(next_link("bad; rel=\"next\"").is_err());
    }
    #[test]
    fn repeated_cursor() {
        let mut c = None;
        let mut s = HashSet::new();
        let p = json!({"pageInfo":{"hasNextPage":true,"endCursor":"a"}});
        assert!(advance(&p, &mut c, &mut s).unwrap());
        assert!(advance(&p, &mut c, &mut s).is_err());
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    async fn server(replies: Vec<String>) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0; 16384];
                let n = socket.read(&mut buf).await.unwrap();
                requests.push(String::from_utf8_lossy(&buf[..n]).to_string());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (base, task)
    }
    fn response(code: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }
    #[tokio::test]
    async fn etag_not_modified() {
        let (base, task) = server(vec![response("304 Not Modified", "", "")]).await;
        let mut c = GithubClient::new(Duration::from_secs(2), None).unwrap();
        c.base = base;
        let r = c.stars("secret", Some("prior")).await.unwrap();
        assert!(r.not_modified);
        assert_eq!(r.etag.as_deref(), Some("prior"));
        let requests = task.await.unwrap();
        assert!(requests[0].to_lowercase().contains("if-none-match: prior"));
    }
    #[tokio::test]
    async fn hostile_next_never_receives_token() {
        let (base, task) = server(vec![response(
            "200 OK",
            "Link: <http://example.invalid/user/starred?page=2>; rel=\"next\"\r\n",
            "[]",
        )])
        .await;
        let mut c = GithubClient::new(Duration::from_secs(2), None).unwrap();
        c.base = base;
        assert!(matches!(
            c.stars("secret", None).await,
            Err(Error::External)
        ));
        assert_eq!(task.await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn malformed_snapshot_is_not_empty() {
        let (base, task) = server(vec![response("200 OK", "", "not json")]).await;
        let mut c = GithubClient::new(Duration::from_secs(2), None).unwrap();
        c.base = base;
        assert!(c.stars("secret", None).await.is_err());
        task.await.unwrap();
    }
    #[tokio::test]
    async fn write_500_is_uncertain_not_retried() {
        let (base, task) = server(vec![response(
            "500 Internal Server Error",
            "",
            "{\"message\":\"secret\"}",
        )])
        .await;
        let mut c = GithubClient::new(Duration::from_secs(2), None).unwrap();
        c.base = base;
        let err = c.star("secret", "a", "b", true).await.unwrap_err();
        assert!(matches!(err, Error::Uncertain));
        assert!(!err.to_string().contains("secret"));
        assert_eq!(task.await.unwrap().len(), 1);
    }
}
