//! Executable source-derived fallback orchestration. The confined worker supplies
//! only transport and CPU extraction primitives, not fallback policy. GitHub
//! credentials go solely to fixed-origin GitHub API requests, never providers.
use crate::{
    Error, Result,
    contracts::{Call, Content, ContentProvider, Insight},
    github::Github,
    normalize::repo_ref,
};
use regex::Regex;
use std::collections::BTreeMap;
use url::Url;
#[derive(Default, Clone)]
pub struct Extraction {
    pub text: String,
    pub title: Option<String>,
}
/// Adapter must execute each request/extraction with the shared content-worker
/// confinement, DNS/redirect/SSRF/size/deadline policies, including llms/provider URLs.
/// Extractors correspond to pinned readability-lxml and trafilatura workers.
pub trait Stages: Send + Sync {
    fn fetch<'a>(
        &'a self,
        user: i64,
        url: &'a str,
        headers: &'a BTreeMap<String, String>,
    ) -> Call<'a, String>;
    fn readability<'a>(&'a self, html: &'a str, url: &'a str) -> Call<'a, Extraction>;
    fn trafilatura<'a>(&'a self, html: &'a str) -> Call<'a, String>;
    /// Pure encoding primitive, no network. Coordinator may implement using
    /// pinned toon_format.encode; default is legacy _to_toon's JSON fallback.
    fn structured(&self, value: serde_json::Value) -> Call<'_, String> {
        Box::pin(async move { serde_json::to_string_pretty(&value).map_err(|_| Error::External) })
    }
}
pub struct FallbackContent<'a> {
    pub stages: &'a dyn Stages,
    pub github: &'a dyn Github,
    pub insight: &'a dyn Insight,
    pub max_chars: usize,
}
fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static Inbox regex")
}
fn strip_html(raw: &str) -> String {
    let removed=regex(r"(?is)<(?:script|style|nav|header|footer|aside|form|iframe)\b[^>]*>.*?</(?:script|style|nav|header|footer|aside|form|iframe)>").replace_all(raw,"");
    let text = regex(r"(?s)<[^>]+>").replace_all(&removed, " ");
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}
fn title(html: &str) -> Option<String> {
    regex(r"(?is)<title[^>]*>(.*?)</title>")
        .captures(html)
        .map(|c| strip_html(&c[1]))
        .filter(|s| !s.is_empty())
}
fn attributes(tag: &str) -> BTreeMap<String, String> {
    regex(r#"(?is)([\w:-]+)\s*=\s*["']([^"']*)["']"#)
        .captures_iter(tag)
        .map(|c| (c[1].to_lowercase(), decode_entities(c[2].trim())))
        .collect()
}
fn opengraph(url: &str, html: &str) -> Content {
    let mut result = Content::default();
    for tag in regex(r"(?is)<meta\b[^>]*>").find_iter(html) {
        let attrs = attributes(tag.as_str());
        let Some(key) = attrs.get("property").or_else(|| attrs.get("name")) else {
            continue;
        };
        let Some(value) = attrs.get("content").filter(|s| !s.is_empty()) else {
            continue;
        };
        match key.to_lowercase().as_str() {
            "og:title" | "twitter:title" => {
                result.title.get_or_insert(value.clone());
            }
            "og:description" | "twitter:description" | "description" => {
                result.summary_seed.get_or_insert(value.clone());
            }
            "og:image" | "twitter:image" | "twitter:image:src" => {
                result.og_image_url.get_or_insert(value.clone());
            }
            "og:site_name" => {
                result.publisher.get_or_insert(value.clone());
            }
            "author" => {
                result.author.get_or_insert(value.clone());
            }
            _ => (),
        }
    }
    result.title = result.title.or_else(|| title(html));
    let base = Url::parse(url).ok();
    for tag in regex(r"(?is)<link\b[^>]*>").find_iter(html) {
        let a = attributes(tag.as_str());
        if a.get("rel")
            .is_some_and(|s| matches!(s.to_lowercase().as_str(), "icon" | "shortcut icon"))
        {
            result.favicon_url = a
                .get("href")
                .and_then(|s| base.as_ref()?.join(s).ok())
                .map(|u| u.to_string());
            if result.favicon_url.is_some() {
                break;
            }
        }
    }
    result.favicon_url = result.favicon_url.or_else(|| {
        base.and_then(|u| u.join("/favicon.ico").ok())
            .map(|u| u.to_string())
    });
    result
}
pub fn strip_provider_chrome(text: &str) -> String {
    let text = regex(r"(?s)\A---\n.*?\n---\n+").replace(text, "");
    let text =
        regex(r"(\A|\n)(#\s+[^\n]+\n+)!\[[^\]]*\]\([^)]+\)\s*\n+").replace(&text, "${1}${2}");
    let text = regex(r"\[!\[[^\]]*\]\([^)]+\)\]\(/[^)]*author[^)]*\)\s*").replace(&text, "");
    let text = regex(r"(?m)^By\s+\[[^\]]+\]\([^)]*author[^)]*\)\s*\n+").replace_all(&text, "");
    let text = regex(r"(?m)^Published[^\n]{0,80}\n+").replace_all(&text, "");
    let text=regex(r"(?m)^(?:Beginning|Born|Currently|Originally|Based|Working|Writing|Having|With over)\b[^\n]*(?:\n[^\n]+)*\n\n").replace(&text, "");
    let stripped =
        regex(r"(?is)(?:\A|\n+)---\n+Powered by \[(?:curl\.md|Jina[^\]]*)\]\([^)]+\)\s*\z")
            .replace(&text, "")
            .trim()
            .to_owned();
    stripped + "\n"
}
fn cap(text: &str, max: usize) -> String {
    let mut result: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        result.push_str("\n\n[Content truncated]")
    }
    result
}
// Explicit nested branches keep ordered, non-fatal fallback stages readable.
#[allow(clippy::collapsible_if)]
impl FallbackContent<'_> {
    async fn fetch(&self, user: i64, url: &str) -> Result<String> {
        self.stages.fetch(user, url, &BTreeMap::new()).await
    }
    async fn github_body(&self, url: &str, token: Option<&str>) -> Result<Extraction> {
        let route = crate::github_content::parse(url).ok_or(Error::Invalid)?;
        let text = match crate::github_content::fetch(self.github, token, &route).await? {
            crate::github_content::Body::Plain(s) => s,
            crate::github_content::Body::Structured { prefix, value } => {
                prefix + &self.stages.structured(value).await?
            }
        };
        Ok(Extraction { text, title: None })
    }
    pub async fn extract_content(&self, user: i64, url: &str) -> Result<Content> {
        if self.max_chars == 0 {
            return Err(Error::Invalid);
        }
        let u = Url::parse(url).map_err(|_| Error::Invalid)?;
        let gh = ["github.com", "api.github.com", "raw.githubusercontent.com"]
            .contains(&u.host_str().unwrap_or(""));
        let token = if gh {
            self.insight.github_token(user, false).await?
        } else {
            None
        };
        let mut extraction = if gh {
            self.github_body(url, token.as_deref())
                .await
                .ok()
                .filter(|r| !r.text.trim().is_empty())
        } else {
            None
        };
        let mut html = None;
        let mut original_html = None;
        let mut fetch_url = url.to_owned();
        if extraction.is_none() {
            if !gh {
                if let Ok(llms) = self
                    .fetch(
                        user,
                        u.join("/llms.txt").map_err(|_| Error::Invalid)?.as_str(),
                    )
                    .await
                {
                    if llms.chars().count() >= 50 {
                        let requested = u.path().trim_end_matches('/');
                        for link in regex(r"\[[^\]]+\]\((https?://[^)]+)\)").captures_iter(&llms) {
                            if let Ok(link_url) = Url::parse(&link[1]) {
                                let path = link_url
                                    .path()
                                    .trim_end_matches('/')
                                    .trim_end_matches("/index.md")
                                    .trim_end_matches(".md");
                                if path == requested {
                                    fetch_url = link_url.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            html = self.fetch(user, &fetch_url).await.ok();
            if !gh {
                if let Some(ref h) = html {
                    if let Ok(r) = self.stages.readability(h, &fetch_url).await {
                        if r.text.chars().count() > 200 {
                            extraction = Some(r)
                        }
                    }
                    if extraction.is_none() {
                        if let Ok(t) = self.stages.trafilatura(h).await {
                            if t.chars().count() > 200 {
                                extraction = Some(Extraction {
                                    text: t,
                                    title: None,
                                })
                            }
                        }
                    }
                }
            }
            if extraction.is_none() {
                for (provider, accept) in [
                    (format!("https://r.jina.ai/{url}"), true),
                    (format!("https://curl.md/{url}"), false),
                ] {
                    let mut headers = BTreeMap::from([("Accept".into(), "text/markdown".into())]);
                    if accept {
                        headers.insert("X-No-Cache".into(), "true".into());
                        headers.insert("X-Return-Format".into(), "markdown".into());
                    }
                    if let Ok(raw) = self.stages.fetch(user, &provider, &headers).await {
                        let stripped = strip_provider_chrome(&raw);
                        if raw.chars().count() > 100
                            && stripped
                                .chars()
                                .filter(|c| !c.is_whitespace() && *c != '-')
                                .count()
                                >= 100
                        {
                            extraction = Some(Extraction {
                                text: stripped,
                                title: None,
                            });
                            break;
                        }
                    }
                }
            }
            if extraction.is_none() && !gh {
                if let Some(ref h) = html {
                    let t = strip_html(h);
                    if h.chars().count() > 200 && t.chars().count() > 100 {
                        extraction = Some(Extraction {
                            text: t,
                            title: title(h),
                        })
                    }
                }
            }
        }
        let mut result = if !gh {
            if fetch_url == url {
                original_html = html;
            }
            if original_html.is_none() {
                original_html = self.fetch(user, url).await.ok();
            }
            opengraph(
                url,
                &original_html
                    .unwrap_or_default()
                    .chars()
                    .take(262144)
                    .collect::<String>(),
            )
        } else {
            Content::default()
        };
        if let Some((owner, repo)) = repo_ref(url) {
            result.title = result.title.or(Some(format!("{owner}/{repo}")));
            if let Ok(meta) = self
                .github
                .get_json(token.as_deref(), &format!("repos/{owner}/{repo}"))
                .await
            {
                result.github_repo_id = meta["id"].as_i64();
                result.summary_seed = meta["description"].as_str().map(str::to_owned);
                result.og_image_url = meta["owner"]["avatar_url"].as_str().map(str::to_owned);
            }
        }
        if let Some(extraction) = extraction {
            result.title = extraction.title.filter(|s| !s.is_empty()).or(result.title);
            result.content_text = Some(cap(&extraction.text, self.max_chars));
            result.crawl_status = Some("success".into());
        } else {
            result.crawl_status = Some("failed".into());
        }
        Ok(result)
    }
}
impl ContentProvider for FallbackContent<'_> {
    fn extract<'a>(&'a self, user: i64, url: &'a str) -> Call<'a, Content> {
        Box::pin(self.extract_content(user, url))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn og_reverse_attributes_and_title() {
        let c = opengraph(
            "https://example.com/a",
            r#"<meta content="Description" property="og:description"><meta name='author' content='Ada'><link href='/icon.png' rel='icon'><title> A &amp; B </title>"#,
        );
        assert_eq!(c.summary_seed.as_deref(), Some("Description"));
        assert_eq!(c.title.as_deref(), Some("A & B"));
        assert_eq!(
            c.favicon_url.as_deref(),
            Some("https://example.com/icon.png")
        );
    }
}
