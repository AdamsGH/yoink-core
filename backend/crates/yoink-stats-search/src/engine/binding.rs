use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommitBinding {
    pub(crate) version: u32,
    pub(crate) tokenizer: String,
    pub(crate) path: String,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) database_id: Vec<u8>,
    pub(crate) revision: i64,
}
