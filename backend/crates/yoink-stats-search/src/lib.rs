//! Native Stats search with an authoritative Turso source and a derived Tantivy index.
//! See docs/backend/stats-search.md for the approved versioned matching contract.
mod engine;
mod service;
mod source;
mod tokenizer;
mod types;

pub use service::{RefreshMode, RefreshReport, SearchError, SearchService};
pub use tokenizer::tokenize;
pub use types::{SearchPage, SearchRequest, StatsMessage};
