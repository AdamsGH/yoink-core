//! Source route parser and projections from Insight's preserved fetch.py.
//! Unknown subpaths intentionally use repo_info; user/org pages have no API
//! route and fall through to providers. No invented compare/release-asset API.
use crate::{Error, Result, github::Github};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use url::Url;
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Passthrough(String),
    Raw(String),
    Repo(String),
    File {
        base: String,
        raw: String,
        path: String,
        reference: String,
    },
    Tree {
        base: String,
        reference: String,
        prefix: String,
    },
    Issue {
        base: String,
        number: i64,
        pr: bool,
    },
    Issues {
        base: String,
        pr: bool,
    },
    Commits {
        base: String,
        branch: Option<String>,
    },
    Releases(String),
}
pub fn parse(url: &str) -> Option<Route> {
    let u = Url::parse(url).ok()?;
    let p: Vec<_> = u.path_segments()?.filter(|s| !s.is_empty()).collect();
    match u.host_str()? {
        "api.github.com" => {
            return Some(Route::Passthrough(format!(
                "{}{}",
                u.path().trim_start_matches('/'),
                u.query().map(|q| format!("?{q}")).unwrap_or_default()
            )));
        }
        "raw.githubusercontent.com" => return (p.len() >= 4).then(|| Route::Raw(p.join("/"))),
        "github.com" => (),
        _ => return None,
    }
    if p.len() < 2 {
        return None;
    }
    let base = format!("repos/{}/{}", p[0], p[1]);
    let section = p.get(2).copied();
    Some(match section {
        Some("blob") if p.len() >= 5 => Route::File {
            base,
            raw: format!("{}/{}/{}/{}", p[0], p[1], p[3], p[4..].join("/")),
            path: p[4..].join("/"),
            reference: p[3].into(),
        },
        Some("tree") => Route::Tree {
            base,
            reference: p.get(3).unwrap_or(&"HEAD").to_string(),
            prefix: p.get(4..).unwrap_or(&[]).join("/"),
        },
        Some("issues" | "pull" | "pulls") => {
            let pr = section != Some("issues");
            if let Some(n) = p
                .get(3)
                .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|n| *n >= 0)
            {
                Route::Issue {
                    base,
                    number: n,
                    pr,
                }
            } else {
                Route::Issues { base, pr }
            }
        }
        Some("commits") => Route::Commits {
            base,
            branch: p.get(3).map(|s| s.to_string()),
        },
        Some("releases") => Route::Releases(base),
        _ => Route::Repo(base),
    })
}
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn required<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or(Error::External)
}
fn array(v: &Value) -> Result<&Vec<Value>> {
    v.as_array().ok_or(Error::External)
}
fn structured(v: &Value) -> Result<String> {
    serde_json::to_string_pretty(v).map_err(|_| Error::External)
}
fn empty<'a>(s: &'a str, default: &'a str) -> &'a str {
    if s.is_empty() { default } else { s }
}
fn py_value(v: &Value, default: &str) -> String {
    if v.is_null() {
        "None".into()
    } else if let Some(s) = v.as_str() {
        s.into()
    } else if v.is_number() {
        v.to_string()
    } else {
        default.into()
    }
}
/// A configured encoder can use the pinned worker's toon_format.encode. This
/// method returns source's documented JSON fallback when that encoder is absent.
pub async fn fetch(g: &dyn Github, token: Option<&str>, route: &Route) -> Result<Body> {
    let plain = match route {
        Route::Raw(path) => g.raw_file(token, path).await?,
        Route::Passthrough(path) => {
            return Ok(Body::Structured {
                prefix: String::new(),
                value: g.get_json(token, path).await?,
            });
        }
        Route::File {
            base,
            raw,
            path,
            reference,
        } => {
            if let Ok(raw) = g.raw_file(token, raw).await {
                return Ok(Body::Plain(raw));
            }
            let d = g
                .get_json(token, &format!("{base}/contents/{path}?ref={reference}"))
                .await?;
            if d["encoding"] == "base64" {
                let raw = required(&d, "content")?
                    .split_whitespace()
                    .collect::<String>();
                String::from_utf8_lossy(&STANDARD.decode(raw).map_err(|_| Error::External)?)
                    .into_owned()
            } else {
                return Ok(Body::Structured {
                    prefix: String::new(),
                    value: d,
                });
            }
        }
        Route::Tree {
            base,
            reference,
            prefix,
        } => {
            let d = g
                .get_json(token, &format!("{base}/git/trees/{reference}?recursive=1"))
                .await?;
            d.get("tree")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|v| prefix.is_empty() || text(v, "path").starts_with(prefix))
                .map(|v| format!("{:<4}  {}", empty(text(v, "type"), "?"), text(v, "path")))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Route::Repo(base) => {
            let d = g.get_json(token, base).await?;
            let mut lines = vec![
                format!("# {}", text(&d, "full_name")),
                text(&d, "description").into(),
                String::new(),
                format!(
                    "Stars: {}  Forks: {}  Open issues: {}",
                    d.get("stargazers_count").unwrap_or(&json!(0)),
                    d.get("forks_count").unwrap_or(&json!(0)),
                    d.get("open_issues_count").unwrap_or(&json!(0))
                ),
                format!(
                    "Language: {}  License: {}",
                    d.get("language")
                        .map(|v| py_value(v, "?"))
                        .unwrap_or("?".into()),
                    d["license"]
                        .get("spdx_id")
                        .map(|v| py_value(v, "?"))
                        .unwrap_or("?".into())
                ),
                format!(
                    "Default branch: {}",
                    d.get("default_branch")
                        .map(|v| py_value(v, "main"))
                        .unwrap_or("main".into())
                ),
                format!("URL: {}", text(&d, "html_url")),
            ];
            if let Ok(rd) = g.get_json(token, &format!("{base}/readme")).await
                && let Some(content) = rd["content"].as_str()
                && let Ok(bytes) = STANDARD.decode(content.split_whitespace().collect::<String>())
            {
                lines.extend([
                    String::new(),
                    "## README".into(),
                    String::from_utf8_lossy(&bytes).chars().take(4000).collect(),
                ]);
            }
            lines.join("\n")
        }
        Route::Issue { base, number, pr } => {
            let segment = if *pr { "pulls" } else { "issues" };
            let api = format!("{base}/{segment}/{number}");
            let d = g.get_json(token, &api).await?;
            let mut lines = vec![
                format!(
                    "# {} #{}: {}",
                    if *pr { "PR" } else { "Issue" },
                    d["number"].as_i64().ok_or(Error::External)?,
                    required(&d, "title")?
                ),
                format!(
                    "State: {}  Author: {}",
                    required(&d, "state")?,
                    required(&d["user"], "login")?
                ),
            ];
            if *pr {
                lines.push(format!(
                    "Base: {} <- Head: {}",
                    required(&d["base"], "ref")?,
                    required(&d["head"], "ref")?
                ));
            }
            lines.extend([String::new(), empty(text(&d, "body"), "(no body)").into()]);
            if let Ok(comments) = g
                .get_json(token, &format!("{api}/comments?per_page=20"))
                .await
            {
                for c in array(&comments)? {
                    lines.extend([
                        String::new(),
                        if *pr {
                            format!(
                                "--- @{} on `{}` ---",
                                required(&c["user"], "login")?,
                                text(c, "path")
                            )
                        } else {
                            format!("--- @{} ---", required(&c["user"], "login")?)
                        },
                        text(c, "body").into(),
                    ]);
                }
            }
            lines.join("\n")
        }
        Route::Issues { base, pr } => {
            let d = g
                .get_json(
                    token,
                    &format!(
                        "{base}/{}?state=open&per_page=30",
                        if *pr { "pulls" } else { "issues" }
                    ),
                )
                .await?;
            let mut items = Vec::new();
            for i in array(&d)? {
                if !pr && i.get("pull_request").is_some() {
                    continue;
                }
                items.push(json!({"number":i["number"].as_i64().ok_or(Error::External)?,"state":required(i,"state")?,"title":required(i,"title")?,"author":required(&i["user"],"login")?}));
            }
            return Ok(Body::Structured {
                prefix: format!(
                    "## {} (open)\n",
                    if *pr { "Pull requests" } else { "Issues" }
                ),
                value: json!(items),
            });
        }
        Route::Commits { base, branch } => {
            let mut api = Url::parse(&format!("https://api.github.com/{base}/commits")).unwrap();
            api.query_pairs_mut().append_pair("per_page", "30");
            if let Some(branch) = branch {
                api.query_pairs_mut().append_pair("sha", branch);
            }
            let d = g
                .get_json(
                    token,
                    &format!(
                        "{}?{}",
                        api.path().trim_start_matches('/'),
                        api.query().unwrap()
                    ),
                )
                .await?;
            let mut items = Vec::new();
            for c in array(&d)? {
                items.push(json!({"sha":required(c,"sha")?.chars().take(8).collect::<String>(),"message":required(&c["commit"],"message")?.lines().next().ok_or(Error::External)?.chars().take(80).collect::<String>(),"author":required(&c["commit"]["author"],"name")?}));
            }
            return Ok(Body::Structured {
                prefix: "## Commits\n".into(),
                value: json!(items),
            });
        }
        Route::Releases(base) => {
            let d = g
                .get_json(token, &format!("{base}/releases?per_page=10"))
                .await?;
            let mut items = Vec::new();
            for r in array(&d)? {
                items.push(json!({"tag":required(r,"tag_name")?,"name":r["name"],"published":text(r,"published_at").chars().take(10).collect::<String>(),"body":text(r,"body").chars().take(300).collect::<String>()}));
            }
            return Ok(Body::Structured {
                prefix: "## Releases\n".into(),
                value: json!(items),
            });
        }
    };
    Ok(Body::Plain(plain))
}
pub enum Body {
    Plain(String),
    Structured { prefix: String, value: Value },
}
impl Body {
    pub fn json_fallback(self) -> Result<String> {
        match self {
            Self::Plain(s) => Ok(s),
            Self::Structured { prefix, value } => Ok(prefix + &structured(&value)?),
        }
    }
}
