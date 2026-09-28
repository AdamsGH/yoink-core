use std::path::Path;

use crate::{
    SearchError, SearchRequest, StatsMessage, engine::CommitBinding, tokenize,
    tokenizer::TOKENIZER_NAME,
};

pub(crate) fn file_binding(path: &Path) -> Result<CommitBinding, SearchError> {
    let path = std::fs::canonicalize(path).map_err(|_| SearchError::Database)?;
    let metadata = std::fs::metadata(&path).map_err(|_| SearchError::Database)?;
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (device, inode) = {
        // Without a stable file identity, force a rebuild on every service open.
        use std::time::{SystemTime, UNIX_EPOCH};
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| SearchError::Database)?;
        let _ = metadata;
        (time.as_secs(), u64::from(time.subsec_nanos()))
    };
    Ok(CommitBinding {
        version: 1,
        tokenizer: TOKENIZER_NAME.into(),
        path: path.to_str().ok_or(SearchError::Database)?.to_owned(),
        device,
        inode,
        database_id: Vec::new(),
        revision: 0,
    })
}

pub(crate) fn validate(request: &SearchRequest) -> Result<(), SearchError> {
    if request.chat_id == 0 {
        return Err(SearchError::InvalidInput("chat_id must be nonzero"));
    }
    if request.limit > 500 {
        return Err(SearchError::InvalidInput("limit must be at most 500"));
    }
    if request.offset > 100_000 {
        return Err(SearchError::InvalidInput("offset must be at most 100000"));
    }
    if matches!((request.from_date, request.to_date), (Some(from), Some(to)) if from > to) {
        return Err(SearchError::InvalidInput(
            "from_date must not follow to_date",
        ));
    }
    if request
        .msg_type
        .as_ref()
        .is_some_and(|kind| kind.len() > 128)
    {
        return Err(SearchError::InvalidInput(
            "msg_type must be at most 128 bytes",
        ));
    }
    if let Some(text) = &request.query {
        if text.len() > 16_384 {
            return Err(SearchError::InvalidInput(
                "query must be at most 16384 bytes",
            ));
        }
        let mut terms = tokenize(text);
        terms.sort_unstable();
        terms.dedup();
        if terms.len() > 64 {
            return Err(SearchError::InvalidInput(
                "query must have at most 64 distinct terms",
            ));
        }
    }
    Ok(())
}

pub(crate) fn decode_message(row: &yoink_db::Row) -> Result<StatsMessage, SearchError> {
    Ok(StatsMessage {
        id: row.try_get("id").map_err(|_| SearchError::Database)?,
        message_id: row
            .try_get("message_id")
            .map_err(|_| SearchError::Database)?,
        chat_id: row.try_get("chat_id").map_err(|_| SearchError::Database)?,
        date: row.try_get("date").map_err(|_| SearchError::Database)?,
        from_user: row
            .try_get("from_user")
            .map_err(|_| SearchError::Database)?,
        text: row.try_get("text").map_err(|_| SearchError::Database)?,
        caption: row.try_get("caption").map_err(|_| SearchError::Database)?,
        msg_type: row.try_get("msg_type").map_err(|_| SearchError::Database)?,
    })
}
