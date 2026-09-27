use serde::{Deserialize, Serialize};
use yoink_domain::{ClipRange, DownloadParams, DownloadRequest, MediaMode};

use crate::StoreError;

#[derive(Serialize, Deserialize)]
struct RequestData {
    actor_id: u64,
    source_url: String,
    media_mode: MediaModeData,
    clip_range: Option<ClipRangeData>,
    max_height: Option<u32>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MediaModeData {
    Video,
    Audio,
}

#[derive(Serialize, Deserialize)]
struct ClipRangeData {
    start_seconds: f64,
    end_seconds: f64,
}

pub(crate) fn encode(request: &DownloadRequest) -> Result<serde_json::Value, StoreError> {
    let data = RequestData {
        actor_id: request.actor_id(),
        source_url: request.source_url().as_str().to_owned(),
        media_mode: match request.media_mode() {
            MediaMode::Video => MediaModeData::Video,
            MediaMode::Audio => MediaModeData::Audio,
        },
        clip_range: request.clip_range().map(|range| ClipRangeData {
            start_seconds: range.start_seconds(),
            end_seconds: range.end_seconds(),
        }),
        max_height: request.params().max_height(),
    };
    serde_json::to_value(data).map_err(|_| StoreError::InvalidStoredData)
}

pub(crate) fn decode(value: serde_json::Value) -> Result<DownloadRequest, StoreError> {
    let data: RequestData =
        serde_json::from_value(value).map_err(|_| StoreError::InvalidStoredData)?;
    let mode = match data.media_mode {
        MediaModeData::Video => MediaMode::Video,
        MediaModeData::Audio => MediaMode::Audio,
    };
    let range = data
        .clip_range
        .map(|range| ClipRange::new(range.start_seconds, range.end_seconds))
        .transpose()
        .map_err(|_| StoreError::InvalidStoredData)?;
    let params = DownloadParams::new(data.max_height).map_err(|_| StoreError::InvalidStoredData)?;
    DownloadRequest::new(data.actor_id, &data.source_url, mode, range, params)
        .map_err(|_| StoreError::InvalidStoredData)
}
