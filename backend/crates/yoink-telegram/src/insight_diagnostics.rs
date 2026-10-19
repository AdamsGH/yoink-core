use serde_json::{Value, json};
use uuid::Uuid;

pub(super) fn record(request: Uuid, chat: i64, message: i32, stage: &str, details: Value) {
    let event = json!({"version":1,"request_id":request,"chat_id":chat,"input_message_id":message,"stage":stage,"details":details});
    tracing::info!("insight_diagnostic={event}");
}
