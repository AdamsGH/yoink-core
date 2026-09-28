use chrono::NaiveDate;

#[derive(Clone, Debug, Default)]
pub struct SearchRequest {
    pub chat_id: i64,
    pub query: Option<String>,
    pub from_user: Option<i64>,
    pub from_date: Option<NaiveDate>,
    pub to_date: Option<NaiveDate>,
    pub msg_type: Option<String>,
    pub limit: u32,
    pub offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatsMessage {
    pub id: i64,
    pub message_id: i64,
    pub chat_id: i64,
    /// UTC microseconds since Unix epoch, including negative timestamps.
    pub date: i64,
    pub from_user: Option<i64>,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub msg_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPage {
    pub items: Vec<StatsMessage>,
    pub total: u64,
}
