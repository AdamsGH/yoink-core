//! Inbox-owned services. Adapters must resolve identity and effective grants afresh.
pub mod bridge;
pub mod catalog;
pub mod content_stages;
pub mod contracts;
pub mod enrich;
pub mod feed;
pub mod github;
pub mod github_content;
pub mod http;
pub mod lists;
pub mod mutations;
pub mod normalize;
pub mod notifications;
pub mod pipeline;
pub mod rules;
pub mod search;
pub mod stars;
pub mod store;
pub mod teams;
pub mod telegram;
pub mod validation;
pub mod worker;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database operation failed")]
    Database(#[from] yoink_db::Error),
    #[error("invalid input")]
    Invalid,
    #[error("not found")]
    NotFound,
    #[error("access denied")]
    Forbidden,
    #[error("GitHub rate limit")]
    RateLimited,
    #[error("external operation failed")]
    External,
    /// Sanitized terminal completion failure; never an upstream body or secret.
    #[error("completion failed: {0}")]
    Completion(&'static str),
    #[error("operation cancelled")]
    Cancelled,
    #[error("GitHub mutation pending reconciliation")]
    MutationPending(String),
    #[error("external mutation outcome unknown; reconcile before retry")]
    Uncertain,
}
pub type Result<T> = std::result::Result<T, Error>;
