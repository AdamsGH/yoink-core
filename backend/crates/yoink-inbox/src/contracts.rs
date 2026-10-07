use crate::Result;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};
pub type Call<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
/// Must consult shared identity, current bans and effective feature grants, not role alone.
pub trait Authorization: Send + Sync {
    fn require<'a>(&'a self, user: i64, feature: &'a str) -> Call<'a, ()>;
    fn require_admin(&self, user: i64) -> Call<'_, ()>;
}
/// Insight owns credential storage and completion routing. Tokens must never reach wire models/logs.
pub trait Insight: Send + Sync {
    fn github_token(&self, user: i64, write: bool) -> Call<'_, Option<String>>;
    fn complete<'a>(&'a self, user: i64, prompt: &'a str) -> Call<'a, String>;
    fn language(&self, user: i64) -> Call<'_, String>;
}
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Content {
    pub summary_seed: Option<String>,
    pub github_repo_id: Option<i64>,
    pub title: Option<String>,
    pub content_text: Option<String>,
    pub og_image_url: Option<String>,
    pub favicon_url: Option<String>,
    pub author: Option<String>,
    pub publisher: Option<String>,
    pub published_at: Option<i64>,
    pub crawl_status: Option<String>,
}
/// Shared confined worker adapter; must enforce existing SSRF/size/time policies.
pub trait ContentProvider: Send + Sync {
    fn extract<'a>(&'a self, user: i64, url: &'a str) -> Call<'a, Content>;
}
pub trait Telegram: Send + Sync {
    fn edit<'a>(&'a self, chat: i64, message: i64, text: &'a str) -> Call<'a, ()>;
}
