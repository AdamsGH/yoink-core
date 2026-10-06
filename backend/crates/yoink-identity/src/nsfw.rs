//! Legacy rule semantics, evaluated from a fresh DB snapshot at every delivery boundary.
use chrono::{DateTime, Utc};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_access::Role;
use yoink_db::query;
use yoink_domain::ContentMetadata;

use crate::{
    IdentityStore,
    admin::{AdminError, admin_auth, finish_admin},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleKind {
    Domain,
    Keyword,
}
#[derive(Clone, Debug, Serialize)]
pub struct NsfwRule {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NsfwRuleInput {
    pub domain: Option<String>,
    pub keyword: Option<String>,
    pub note: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NsfwImport {
    #[serde(default)]
    pub domains: Vec<NsfwRuleInput>,
    #[serde(default)]
    pub keywords: Vec<NsfwRuleInput>,
}
#[derive(Clone, Debug, Serialize)]
pub struct NsfwCheck {
    pub url: String,
    pub is_nsfw: bool,
    pub matched_domain: Option<String>,
    pub matched_keyword: Option<String>,
    pub matched_keywords: Vec<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentDecision {
    pub blocked: bool,
    pub spoiler: bool,
}

struct Rules {
    domains: Vec<String>,
    keywords: Vec<(String, Regex, Regex)>,
}
fn normalize(kind: RuleKind, value: &str) -> Result<String, AdminError> {
    let s = value.to_lowercase();
    let s = if kind == RuleKind::Domain {
        s.strip_prefix("www.").unwrap_or(&s).to_owned()
    } else {
        s
    };
    if (kind == RuleKind::Domain && s.trim() != s)
        || s.is_empty()
        || s.chars().any(char::is_control)
        || s.chars().count() > if kind == RuleKind::Domain { 253 } else { 128 }
    {
        return Err(AdminError::InvalidInput);
    }
    if kind == RuleKind::Domain {
        let parsed =
            url::Url::parse(&format!("https://{s}/")).map_err(|_| AdminError::InvalidInput)?;
        if parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AdminError::InvalidInput);
        }
        let host = &parsed[url::Position::BeforeHost..url::Position::AfterHost];
        if !host.starts_with('[')
            && host
                .split('.')
                .any(|p| p.is_empty() || p.len() > 63 || p.starts_with('-') || p.ends_with('-'))
        {
            return Err(AdminError::InvalidInput);
        }
        let suffix = if s.starts_with('[') {
            s.split_once(']').map(|(_, tail)| tail).unwrap_or("")
        } else {
            s.rsplit_once(':')
                .map(|(_, port)| &s[s.len() - port.len() - 1..])
                .unwrap_or("")
        };
        if !suffix.is_empty()
            && suffix
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .is_none()
        {
            return Err(AdminError::InvalidInput);
        }
        return Ok(format!("{host}{suffix}"));
    }
    Ok(s)
}
// Python re.IGNORECASE extends ASCII i matching to dotted/dotless I.
// Rust regex simple folding already handles long s and Kelvin k, but not these.
fn escaped_keyword(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == 'i' {
                "[iIİı]".to_owned()
            } else {
                regex::escape(&c.to_string())
            }
        })
        .collect()
}
fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn boundary_match(re: &Regex, text: &str, ascii: bool) -> bool {
    re.find_iter(text).any(|m| {
        let is_word = |c: char| {
            if ascii {
                c.is_ascii_alphanumeric() || matches!(c, 'İ' | 'ı' | 'ſ' | 'K')
            } else {
                word(c)
            }
        };
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if ascii {
            !before.is_some_and(is_word) && !after.is_some_and(is_word)
        } else {
            before.is_some_and(is_word) != m.as_str().chars().next().is_some_and(is_word)
                && after.is_some_and(is_word) != m.as_str().chars().next_back().is_some_and(is_word)
        }
    })
}
impl Rules {
    fn compile(domains: Vec<String>, keywords: Vec<String>) -> Result<Self, AdminError> {
        let domains = domains
            .into_iter()
            .map(|v| normalize(RuleKind::Domain, &v).map_err(|_| AdminError::Unavailable))
            .collect::<Result<Vec<_>, _>>()?;
        let mut patterns = Vec::new();
        for keyword in keywords {
            let keyword =
                normalize(RuleKind::Keyword, &keyword).map_err(|_| AdminError::Unavailable)?;
            let url = keyword
                .split_whitespace()
                .map(escaped_keyword)
                .collect::<Vec<_>>()
                .join("[^A-Za-z0-9İı]+");
            if url.is_empty() {
                return Err(AdminError::Unavailable);
            }
            let compile = |s: &str| {
                RegexBuilder::new(s)
                    .case_insensitive(true)
                    .size_limit(100_000)
                    .build()
                    .map_err(|_| AdminError::Unavailable)
            };
            patterns.push((
                keyword.clone(),
                compile(&url)?,
                compile(&escaped_keyword(&keyword))?,
            ));
        }
        Ok(Self {
            domains,
            keywords: patterns,
        })
    }
    fn check(&self, source: &str, metadata: &[ContentMetadata]) -> Result<NsfwCheck, AdminError> {
        let url = url::Url::parse(source).map_err(|_| AdminError::InvalidInput)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || source.len() > 4096
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(AdminError::InvalidInput);
        }
        let host = url.host_str().unwrap_or_default().to_lowercase();
        let authority = url
            .port_or_known_default()
            .map(|port| format!("{host}:{port}"));
        let matched_domain = self
            .domains
            .iter()
            .find(|d| {
                host == **d
                    || host.ends_with(&format!(".{d}"))
                    || authority
                        .as_ref()
                        .is_some_and(|a| a == *d || a.ends_with(&format!(".{d}")))
            })
            .cloned();
        if metadata.iter().any(|m| !m.valid()) {
            return Err(AdminError::Unavailable);
        }
        let mut matched_keywords = Vec::new();
        for (keyword, url_re, text_re) in &self.keywords {
            if boundary_match(url_re, source, true)
                || metadata
                    .iter()
                    .any(|m| boundary_match(text_re, &m.prose(), false))
            {
                matched_keywords.push(keyword.clone());
            }
        }
        Ok(NsfwCheck {
            url: source.into(),
            is_nsfw: matched_domain.is_some() || !matched_keywords.is_empty(),
            matched_domain,
            matched_keyword: matched_keywords.first().cloned(),
            matched_keywords,
        })
    }
}
async fn load(tx: &Transaction<'_>, kind: RuleKind) -> Result<Vec<NsfwRule>, AdminError> {
    let sql = match kind {
        RuleKind::Domain => include_str!("nsfw_domains.sql"),
        RuleKind::Keyword => include_str!("nsfw_keywords.sql"),
    };
    let rows = query(sql)
        .fetch_all(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?;
    if rows.len() > 10000 {
        return Err(AdminError::Unavailable);
    }
    rows.into_iter()
        .map(|r| {
            let value: String = r.try_get(1).map_err(|_| AdminError::Unavailable)?;
            Ok(NsfwRule {
                id: r.try_get(0).map_err(|_| AdminError::Unavailable)?,
                domain: (kind == RuleKind::Domain).then(|| value.clone()),
                keyword: (kind == RuleKind::Keyword).then_some(value),
                note: r.try_get(2).map_err(|_| AdminError::Unavailable)?,
                created_at: r.try_get(3).map_err(|_| AdminError::Unavailable)?,
            })
        })
        .collect()
}
async fn rules(tx: &Transaction<'_>) -> Result<Rules, AdminError> {
    Rules::compile(
        load(tx, RuleKind::Domain)
            .await?
            .into_iter()
            .filter_map(|r| r.domain)
            .collect(),
        load(tx, RuleKind::Keyword)
            .await?
            .into_iter()
            .filter_map(|r| r.keyword)
            .collect(),
    )
}
async fn revision(tx: &Transaction<'_>) -> Result<i64, AdminError> {
    let value: String = query(include_str!("nsfw_revision.sql"))
        .fetch_one(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .try_get(0)
        .map_err(|_| AdminError::Unavailable)?;
    let parsed = value.parse::<i64>().map_err(|_| AdminError::Unavailable)?;
    if parsed < 0 || parsed == i64::MAX || parsed.to_string() != value {
        return Err(AdminError::Unavailable);
    }
    Ok(parsed)
}
async fn authorize(
    store: &IdentityStore,
    tx: &Transaction<'_>,
    actor: i64,
) -> Result<(), AdminError> {
    if admin_auth(tx, actor, store.owner_id, false, false).await? < Role::Moderator {
        return Err(AdminError::Forbidden);
    }
    Ok(())
}
impl IdentityStore {
    /// Missing settings use the legacy private blur default. Missing group policy fails closed.
    pub async fn content_policy(
        &self,
        actor: i64,
        chat: i64,
        source: &str,
        metadata: &[ContentMetadata],
    ) -> Result<ContentDecision, AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            let classified = rules(&tx).await?.check(source, metadata)?.is_nsfw;
            let row = query(include_str!("nsfw_policy.sql"))
                .bind(actor)
                .bind(chat)
                .fetch_one(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            let blur: i64 = row.try_get(0).map_err(|_| AdminError::Unavailable)?;
            let allowed: Option<i64> = row.try_get(1).map_err(|_| AdminError::Unavailable)?;
            if !matches!(blur, 0 | 1)
                || (chat < 0 && !matches!(allowed, Some(0 | 1)))
                || (chat > 0 && chat != actor)
                || chat == 0
            {
                return Err(AdminError::Unavailable);
            }
            Ok(ContentDecision {
                blocked: classified && chat < 0 && allowed != Some(1),
                spoiler: classified && chat > 0 && blur == 1,
            })
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn nsfw_list(
        &self,
        actor: i64,
        kind: RuleKind,
    ) -> Result<(Vec<NsfwRule>, i64), AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            authorize(self, &tx, actor).await?;
            Ok((load(&tx, kind).await?, revision(&tx).await?))
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn nsfw_export(
        &self,
        actor: i64,
    ) -> Result<(Vec<NsfwRule>, Vec<NsfwRule>, i64), AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            authorize(self, &tx, actor).await?;
            Ok((
                load(&tx, RuleKind::Domain).await?,
                load(&tx, RuleKind::Keyword).await?,
                revision(&tx).await?,
            ))
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn nsfw_check(&self, actor: i64, source: &str) -> Result<NsfwCheck, AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            authorize(self, &tx, actor).await?;
            rules(&tx).await?.check(source, &[])
        }
        .await;
        finish_admin(tx, result).await
    }
    /// A mutation is authorized again inside its IMMEDIATE transaction. Optional revision
    /// preserves unversioned legacy clients; supplied revisions are never ignored.
    pub async fn nsfw_mutate(
        &self,
        actor: i64,
        kind: RuleKind,
        id: Option<i64>,
        input: Option<&NsfwRuleInput>,
        expected: Option<i64>,
    ) -> Result<Option<NsfwRule>, AdminError> {
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            authorize(self, &tx, actor).await?;
            revision(&tx).await?;
            if let Some(v) = expected
                && (v < 0 || v != revision(&tx).await?)
            {
                return Err(AdminError::Conflict);
            }
            let old = if let Some(id) = id {
                Some(
                    load(&tx, kind)
                        .await?
                        .into_iter()
                        .find(|r| r.id == id)
                        .ok_or(AdminError::NotFound)?,
                )
            } else {
                None
            };
            let result = if let Some(input) = input {
                let raw = match kind {
                    RuleKind::Domain => input
                        .domain
                        .as_deref()
                        .or_else(|| old.as_ref().and_then(|r| r.domain.as_deref())),
                    RuleKind::Keyword => input
                        .keyword
                        .as_deref()
                        .or_else(|| old.as_ref().and_then(|r| r.keyword.as_deref())),
                }
                .ok_or(AdminError::InvalidInput)?;
                if (kind == RuleKind::Domain && input.keyword.is_some())
                    || (kind == RuleKind::Keyword && input.domain.is_some())
                    || input
                        .note
                        .as_ref()
                        .is_some_and(|s| s.chars().count() > 256 || s.chars().any(char::is_control))
                {
                    return Err(AdminError::InvalidInput);
                }
                let value = normalize(kind, raw)?;
                let note = match input.note.as_deref() {
                    Some("") => None,
                    Some(note) => Some(note),
                    None => old.as_ref().and_then(|r| r.note.as_deref()),
                };
                let sql = match (kind, id) {
                    (RuleKind::Domain, None) => include_str!("nsfw_domain_insert.sql"),
                    (RuleKind::Keyword, None) => include_str!("nsfw_keyword_insert.sql"),
                    (RuleKind::Domain, Some(_)) => include_str!("nsfw_domain_update.sql"),
                    (RuleKind::Keyword, Some(_)) => include_str!("nsfw_keyword_update.sql"),
                };
                let row = query(sql)
                    .bind(value)
                    .bind(note)
                    .bind(id)
                    .fetch_optional(&*tx)
                    .await
                    .map_err(|_| AdminError::Conflict)?
                    .ok_or(AdminError::NotFound)?;
                let changed: i64 = row.try_get(0).map_err(|_| AdminError::Unavailable)?;
                Some(
                    load(&tx, kind)
                        .await?
                        .into_iter()
                        .find(|r| r.id == changed)
                        .ok_or(AdminError::Unavailable)?,
                )
            } else {
                let id = id.ok_or(AdminError::InvalidInput)?;
                query(match kind {
                    RuleKind::Domain => include_str!("nsfw_domain_delete.sql"),
                    RuleKind::Keyword => include_str!("nsfw_keyword_delete.sql"),
                })
                .bind(id)
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
                None
            };
            rules(&tx).await?;
            query(include_str!("nsfw_revision_advance.sql"))
                .execute(&*tx)
                .await
                .map_err(|_| AdminError::Unavailable)?;
            Ok(result)
        }
        .await;
        finish_admin(tx, result).await
    }
    pub async fn nsfw_import(
        &self,
        actor: i64,
        input: &NsfwImport,
        expected: Option<i64>,
    ) -> Result<(usize, usize), AdminError> {
        if input.domains.len() + input.keywords.len() > 10000 {
            return Err(AdminError::InvalidInput);
        }
        let mut c = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result =
            async {
                authorize(self, &tx, actor).await?;
                revision(&tx).await?;
                if let Some(v) = expected
                    && (v < 0 || v != revision(&tx).await?)
                {
                    return Err(AdminError::Conflict);
                }
                let mut counts = [0, 0];
                for (index, kind, list) in [
                    (0, RuleKind::Domain, &input.domains),
                    (1, RuleKind::Keyword, &input.keywords),
                ] {
                    for item in list {
                        let raw = match kind {
                            RuleKind::Domain => item.domain.as_deref(),
                            RuleKind::Keyword => item.keyword.as_deref(),
                        }
                        .ok_or(AdminError::InvalidInput)?;
                        if item.note.as_ref().is_some_and(|s| {
                            s.chars().count() > 256 || s.chars().any(char::is_control)
                        }) || (kind == RuleKind::Domain && item.keyword.is_some())
                            || (kind == RuleKind::Keyword && item.domain.is_some())
                        {
                            return Err(AdminError::InvalidInput);
                        }
                        let q = query(match kind {
                            RuleKind::Domain => include_str!("nsfw_domain_import.sql"),
                            RuleKind::Keyword => include_str!("nsfw_keyword_import.sql"),
                        })
                        .bind(normalize(kind, raw)?)
                        .bind(item.note.as_deref());
                        counts[index] += q
                            .execute(&*tx)
                            .await
                            .map_err(|_| AdminError::Unavailable)?
                            .rows_affected() as usize;
                    }
                }
                rules(&tx).await?;
                query(include_str!("nsfw_revision_advance.sql"))
                    .execute(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?;
                Ok((counts[0], counts[1]))
            }
            .await;
        finish_admin(tx, result).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nsfw_legacy_domain_delimiter_and_unicode_prose() {
        let rules = Rules::compile(
            vec!["adult.example".into()],
            vec!["sex".into(), "adult content".into(), "эротика".into()],
        )
        .unwrap();
        for url in [
            "https://sub.adult.example/x",
            "https://x.example/adult-content",
            "https://x.example/?q=sex",
            "https://x.example/sex_video",
        ] {
            assert!(rules.check(url, &[]).unwrap().is_nsfw);
        }
        for url in [
            "https://notadult.example/",
            "https://x.example/sussex",
            "https://x.example/sexual",
        ] {
            assert!(!rules.check(url, &[]).unwrap().is_nsfw);
        }
        for field in 0..4 {
            let mut m = ContentMetadata::default();
            match field {
                0 => m.title = "ЭРОТИКА!".into(),
                1 => m.description = "adult content".into(),
                2 => m.tags = vec!["sex".into()],
                _ => m.categories = vec!["эротика".into()],
            };
            assert!(rules.check("https://x.example/", &[m]).unwrap().is_nsfw);
        }
        let m = ContentMetadata {
            title: "суперэротика sex_foo".into(),
            ..Default::default()
        };
        assert!(!rules.check("https://x.example/", &[m]).unwrap().is_nsfw);
        assert!(Rules::compile(vec!["bad/path".into()], vec![]).is_err());
        let unicode_case = Rules::compile(vec![], vec!["indecent".into(), "sex".into()]).unwrap();
        assert!(
            unicode_case
                .check(
                    "https://x.example/",
                    &[ContentMetadata {
                        title: "İndecent".into(),
                        ..Default::default()
                    }]
                )
                .unwrap()
                .is_nsfw
        );
        assert!(
            !unicode_case
                .check("https://x.example/Ksex", &[])
                .unwrap()
                .is_nsfw
        );
        let authority = Rules::compile(
            vec![
                "пример.рф".into(),
                "adult.example:8443".into(),
                "[::1]".into(),
            ],
            vec![],
        )
        .unwrap();
        for source in [
            "https://sub.пример.рф/",
            "https://sub.adult.example:8443/",
            "https://[::1]/",
        ] {
            assert!(authority.check(source, &[]).unwrap().is_nsfw);
        }
    }
}
