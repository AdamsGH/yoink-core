use serde::{Deserialize, Serialize};
use yoink_domain::{
    AudioCodec, ClipRange, Container, DeliveryTarget, DownloadParams, DownloadPreferenceOverrides,
    DownloadRequest, Engine, MediaMode, PlaylistRange, SubtitleOptions, VideoCodec,
};

use crate::StoreError;

mod network;
use network::NetworkSelectionData;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestData {
    actor_id: u64,
    source_url: String,
    media_mode: MediaModeData,
    #[serde(default)]
    clip_range: Option<ClipRangeData>,
    #[serde(default)]
    params: DownloadParamsData,
    #[serde(default)]
    target: Option<DeliveryTargetData>,
    #[serde(default)]
    network: NetworkSelectionData,
    // Rows persisted by migration 0001 used this top-level field.
    #[serde(default)]
    max_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preference_overrides: Option<PreferenceOverridesData>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferenceOverridesData {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_override_height"
    )]
    max_height: Option<Option<u32>>,
    #[serde(default)]
    video_codec: Option<VideoCodecData>,
    #[serde(default)]
    audio_codec: Option<AudioCodecData>,
    #[serde(default)]
    container: Option<ContainerData>,
    #[serde(default)]
    send_as_file: Option<bool>,
    #[serde(default)]
    gallery_zip: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subtitle_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subtitle_automatic: Option<bool>,
}

fn deserialize_override_height<'de, D>(deserializer: D) -> Result<Option<Option<u32>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u32>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MediaModeData {
    Video,
    Audio,
    Image,
    Subtitles,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VideoCodecData {
    H264,
    H265,
    Av1,
    Vp9,
    Any,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AudioCodecData {
    Best,
    Aac,
    Mp3,
    Opus,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ContainerData {
    Mp4,
    Mkv,
    Webm,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EngineData {
    Auto,
    YtDlp,
    Gallery,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DownloadParamsData {
    #[serde(default)]
    max_height: Option<u32>,
    #[serde(default = "default_video_codec")]
    video_codec: VideoCodecData,
    #[serde(default = "default_audio_codec")]
    audio_codec: AudioCodecData,
    #[serde(default = "default_container")]
    container: ContainerData,
    #[serde(default = "default_engine")]
    engine: EngineData,
    #[serde(default)]
    playlist: Option<PlaylistRangeData>,
    #[serde(default)]
    clips: Vec<ClipRangeData>,
    #[serde(default)]
    send_as_file: bool,
    #[serde(default)]
    gallery_zip: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subtitles: Option<SubtitleOptionsData>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubtitleOptionsData {
    language: String,
    automatic: bool,
}
fn default_video_codec() -> VideoCodecData {
    VideoCodecData::H264
}
fn default_audio_codec() -> AudioCodecData {
    AudioCodecData::Best
}
fn default_container() -> ContainerData {
    ContainerData::Mp4
}
fn default_engine() -> EngineData {
    EngineData::Auto
}
impl Default for DownloadParamsData {
    fn default() -> Self {
        Self {
            max_height: None,
            video_codec: default_video_codec(),
            audio_codec: default_audio_codec(),
            container: default_container(),
            engine: default_engine(),
            playlist: None,
            clips: Vec::new(),
            send_as_file: false,
            gallery_zip: false,
            subtitles: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryTargetData {
    bot_id: i64,
    chat_id: i64,
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default)]
    reply_to_message_id: Option<i32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaylistRangeData {
    start: i32,
    #[serde(default)]
    end: Option<i32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipRangeData {
    start_seconds: f64,
    end_seconds: f64,
}

pub(crate) fn encode(request: &DownloadRequest) -> Result<serde_json::Value, StoreError> {
    let p = request.params();
    let data = RequestData {
        actor_id: request.actor_id(),
        source_url: request.source_url().as_str().to_owned(),
        media_mode: match request.media_mode() {
            MediaMode::Video => MediaModeData::Video,
            MediaMode::Audio => MediaModeData::Audio,
            MediaMode::Image => MediaModeData::Image,
            MediaMode::Subtitles => MediaModeData::Subtitles,
        },
        clip_range: request.clip_range().map(|r| ClipRangeData {
            start_seconds: r.start_seconds(),
            end_seconds: r.end_seconds(),
        }),
        params: DownloadParamsData {
            max_height: p.max_height(),
            video_codec: match p.video_codec() {
                VideoCodec::H264 => VideoCodecData::H264,
                VideoCodec::H265 => VideoCodecData::H265,
                VideoCodec::Av1 => VideoCodecData::Av1,
                VideoCodec::Vp9 => VideoCodecData::Vp9,
                VideoCodec::Any => VideoCodecData::Any,
            },
            audio_codec: match p.audio_codec() {
                AudioCodec::Best => AudioCodecData::Best,
                AudioCodec::Aac => AudioCodecData::Aac,
                AudioCodec::Mp3 => AudioCodecData::Mp3,
                AudioCodec::Opus => AudioCodecData::Opus,
            },
            container: match p.container() {
                Container::Mp4 => ContainerData::Mp4,
                Container::Mkv => ContainerData::Mkv,
                Container::Webm => ContainerData::Webm,
            },
            engine: match p.engine() {
                Engine::Auto => EngineData::Auto,
                Engine::YtDlp => EngineData::YtDlp,
                Engine::Gallery => EngineData::Gallery,
            },
            playlist: p.playlist().map(|r| PlaylistRangeData {
                start: r.start(),
                end: r.end(),
            }),
            clips: p
                .clips()
                .iter()
                .map(|r| ClipRangeData {
                    start_seconds: r.start_seconds(),
                    end_seconds: r.end_seconds(),
                })
                .collect(),
            send_as_file: p.send_as_file(),
            gallery_zip: p.gallery_zip(),
            subtitles: p.subtitles().map(|s| SubtitleOptionsData {
                language: s.language().into(),
                automatic: s.automatic(),
            }),
        },
        target: request.target().map(|t| DeliveryTargetData {
            bot_id: t.bot_id(),
            chat_id: t.chat_id(),
            thread_id: t.thread_id(),
            reply_to_message_id: t.reply_to_message_id(),
        }),
        network: request.network().into(),
        max_height: None,
        preference_overrides: request
            .preference_overrides()
            .map(|o| PreferenceOverridesData {
                max_height: o.max_height,
                video_codec: o.video_codec.map(|v| match v {
                    VideoCodec::H264 => VideoCodecData::H264,
                    VideoCodec::H265 => VideoCodecData::H265,
                    VideoCodec::Av1 => VideoCodecData::Av1,
                    VideoCodec::Vp9 => VideoCodecData::Vp9,
                    VideoCodec::Any => VideoCodecData::Any,
                }),
                audio_codec: o.audio_codec.map(|v| match v {
                    AudioCodec::Best => AudioCodecData::Best,
                    AudioCodec::Aac => AudioCodecData::Aac,
                    AudioCodec::Mp3 => AudioCodecData::Mp3,
                    AudioCodec::Opus => AudioCodecData::Opus,
                }),
                container: o.container.map(|v| match v {
                    Container::Mp4 => ContainerData::Mp4,
                    Container::Mkv => ContainerData::Mkv,
                    Container::Webm => ContainerData::Webm,
                }),
                send_as_file: o.send_as_file,
                gallery_zip: o.gallery_zip,
                subtitle_language: o.subtitle_language.clone(),
                subtitle_automatic: o.subtitle_automatic,
            }),
    };
    serde_json::to_value(data).map_err(|_| StoreError::InvalidStoredData)
}

pub(crate) fn decode(value: serde_json::Value) -> Result<DownloadRequest, StoreError> {
    let data: RequestData =
        serde_json::from_value(value).map_err(|_| StoreError::InvalidStoredData)?;
    let mode = match data.media_mode {
        MediaModeData::Video => MediaMode::Video,
        MediaModeData::Audio => MediaMode::Audio,
        MediaModeData::Image => MediaMode::Image,
        MediaModeData::Subtitles => MediaMode::Subtitles,
    };
    let range = data
        .clip_range
        .map(|r| ClipRange::new(r.start_seconds, r.end_seconds))
        .transpose()
        .map_err(|_| StoreError::InvalidStoredData)?;
    let mut p = data.params;
    if p.max_height.is_none() {
        p.max_height = data.max_height;
    }
    let mut params = DownloadParams::new(p.max_height)
        .map_err(|_| StoreError::InvalidStoredData)?
        .with_video_codec(match p.video_codec {
            VideoCodecData::H264 => VideoCodec::H264,
            VideoCodecData::H265 => VideoCodec::H265,
            VideoCodecData::Av1 => VideoCodec::Av1,
            VideoCodecData::Vp9 => VideoCodec::Vp9,
            VideoCodecData::Any => VideoCodec::Any,
        })
        .with_audio_codec(match p.audio_codec {
            AudioCodecData::Best => AudioCodec::Best,
            AudioCodecData::Aac => AudioCodec::Aac,
            AudioCodecData::Mp3 => AudioCodec::Mp3,
            AudioCodecData::Opus => AudioCodec::Opus,
        })
        .with_container(match p.container {
            ContainerData::Mp4 => Container::Mp4,
            ContainerData::Mkv => Container::Mkv,
            ContainerData::Webm => Container::Webm,
        })
        .with_engine(match p.engine {
            EngineData::Auto => Engine::Auto,
            EngineData::YtDlp => Engine::YtDlp,
            EngineData::Gallery => Engine::Gallery,
        })
        .with_playlist(
            p.playlist
                .map(|r| {
                    PlaylistRange::new(r.start, r.end).map_err(|_| StoreError::InvalidStoredData)
                })
                .transpose()?,
        );
    let clips = p
        .clips
        .drain(..)
        .map(|r| {
            ClipRange::new(r.start_seconds, r.end_seconds)
                .map_err(|_| StoreError::InvalidStoredData)
        })
        .collect::<Result<Vec<_>, _>>()?;
    params = params
        .with_clips(clips)
        .map_err(|_| StoreError::InvalidStoredData)?
        .with_send_as_file(p.send_as_file)
        .with_gallery_zip(p.gallery_zip)
        .with_subtitles(
            p.subtitles
                .map(|s| SubtitleOptions::new(&s.language, s.automatic))
                .transpose()
                .map_err(|_| StoreError::InvalidStoredData)?,
        );
    let mut request = DownloadRequest::new(data.actor_id, &data.source_url, mode, range, params)
        .map_err(|_| StoreError::InvalidStoredData)?;
    if let Some(t) = data.target {
        let target = DeliveryTarget::new(t.bot_id, t.chat_id, t.thread_id, t.reply_to_message_id)
            .map_err(|_| StoreError::InvalidStoredData)?;
        request = request
            .with_target(target)
            .map_err(|_| StoreError::InvalidStoredData)?;
    }
    if let Some(o) = data.preference_overrides {
        if o.max_height
            .flatten()
            .is_some_and(|height| height == 0 || height > 100_000)
        {
            return Err(StoreError::InvalidStoredData);
        }
        request = request.with_preference_overrides(DownloadPreferenceOverrides {
            max_height: o.max_height,
            video_codec: o.video_codec.map(|v| match v {
                VideoCodecData::H264 => VideoCodec::H264,
                VideoCodecData::H265 => VideoCodec::H265,
                VideoCodecData::Av1 => VideoCodec::Av1,
                VideoCodecData::Vp9 => VideoCodec::Vp9,
                VideoCodecData::Any => VideoCodec::Any,
            }),
            audio_codec: o.audio_codec.map(|v| match v {
                AudioCodecData::Best => AudioCodec::Best,
                AudioCodecData::Aac => AudioCodec::Aac,
                AudioCodecData::Mp3 => AudioCodec::Mp3,
                AudioCodecData::Opus => AudioCodec::Opus,
            }),
            container: o.container.map(|v| match v {
                ContainerData::Mp4 => Container::Mp4,
                ContainerData::Mkv => Container::Mkv,
                ContainerData::Webm => Container::Webm,
            }),
            send_as_file: o.send_as_file,
            gallery_zip: o.gallery_zip,
            subtitle_language: o.subtitle_language,
            subtitle_automatic: o.subtitle_automatic,
        });
    }
    request
        .with_network(data.network.try_into()?)
        .map_err(|_| StoreError::InvalidStoredData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoink_domain::{DeliveryTarget, DownloadParams};

    fn request() -> DownloadRequest {
        let params = DownloadParams::default()
            .with_video_codec(VideoCodec::Av1)
            .with_audio_codec(AudioCodec::Opus)
            .with_container(Container::Webm)
            .with_engine(Engine::Gallery)
            .with_playlist(Some(PlaylistRange::new(2, Some(4)).unwrap()))
            .with_clips(vec![ClipRange::new(1.0, 2.0).unwrap()])
            .unwrap()
            .with_send_as_file(true)
            .with_gallery_zip(true);
        DownloadRequest::new(
            7,
            "https://example.com/media",
            MediaMode::Image,
            None,
            params,
        )
        .unwrap()
        .with_target(DeliveryTarget::new(19, 7, Some(4), Some(8)).unwrap())
        .unwrap()
    }

    #[test]
    fn all_domain_parameters_round_trip_without_secrets() {
        let original = request();
        let json = encode(&original).unwrap();
        assert!(!json.to_string().contains("token"));
        assert_eq!(decode(json).unwrap(), original);
    }

    #[test]
    fn old_rows_decode_with_parameter_defaults() {
        let old = serde_json::json!({"actor_id":7,"source_url":"https://example.com/a","media_mode":"video","clip_range":null,"max_height":720});
        let decoded = decode(old).unwrap();
        assert_eq!(decoded.params().max_height(), Some(720));
        assert_eq!(decoded.params().video_codec(), VideoCodec::H264);
        assert_eq!(decoded.media_mode(), MediaMode::Video);
    }

    #[test]
    fn preference_override_metadata_round_trips_absent_and_explicit_best() {
        use yoink_domain::DownloadPreferenceOverrides;
        let absent = request().with_preference_overrides(DownloadPreferenceOverrides::default());
        let encoded_absent = encode(&absent).unwrap();
        assert!(
            encoded_absent["preference_overrides"]
                .get("max_height")
                .is_none()
        );
        assert_eq!(decode(encoded_absent).unwrap(), absent);

        let explicit_best = request().with_preference_overrides(DownloadPreferenceOverrides {
            max_height: Some(None),
            ..Default::default()
        });
        let encoded_best = encode(&explicit_best).unwrap();
        assert!(
            encoded_best["preference_overrides"]
                .get("max_height")
                .unwrap()
                .is_null()
        );
        assert_eq!(decode(encoded_best).unwrap(), explicit_best);

        let plain = request();
        let encoded = encode(&plain).unwrap();
        assert!(encoded.get("preference_overrides").is_none());
        assert_eq!(decode(encoded).unwrap(), plain);

        let old = serde_json::json!({"actor_id":7,"source_url":"https://example.com/a","media_mode":"video"});
        let decoded = decode(old).unwrap();
        assert!(decoded.preference_overrides().is_none());
    }

    #[test]
    fn malformed_preference_override_metadata_is_rejected() {
        let base = serde_json::json!({"actor_id":7,"source_url":"https://example.com/a","media_mode":"video"});
        for height in [serde_json::json!(0), serde_json::json!(100001)] {
            let mut invalid = base.clone();
            invalid["preference_overrides"] = serde_json::json!({"max_height":height});
            assert_eq!(decode(invalid).unwrap_err(), StoreError::InvalidStoredData);
        }
        let mut unknown = base;
        unknown["preference_overrides"] = serde_json::json!({"unexpected":true});
        assert_eq!(decode(unknown).unwrap_err(), StoreError::InvalidStoredData);
        let mut bad_type = serde_json::json!({"actor_id":7,"source_url":"https://example.com/a","media_mode":"video"});
        bad_type["preference_overrides"] = serde_json::json!({"video_codec":"unsafe"});
        assert_eq!(decode(bad_type).unwrap_err(), StoreError::InvalidStoredData);
    }

    #[test]
    fn unknown_fields_and_invalid_parameters_are_rejected() {
        let base = serde_json::json!({"actor_id":7,"source_url":"https://example.com/a","media_mode":"video"});
        let mut unknown = base.clone();
        unknown["surprise"] = serde_json::json!(true);
        assert_eq!(decode(unknown).unwrap_err(), StoreError::InvalidStoredData);
        let mut invalid = base;
        invalid["params"] = serde_json::json!({"max_height":0});
        assert_eq!(decode(invalid).unwrap_err(), StoreError::InvalidStoredData);
    }
}
