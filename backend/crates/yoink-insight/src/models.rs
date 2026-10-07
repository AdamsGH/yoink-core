//! Native module additions; imported product rows retain their existing schema.
#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub cache_key: String,
    pub user_id: i64,
    pub result: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Clone, Debug)]
pub struct Reservation {
    pub id: String,
    pub user_id: i64,
    pub command: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}
