use std::fmt;

use url::Url;
use yoink_domain::{
    AudioCodec, ClipRange, Container, DownloadPreferenceOverrides, Engine, MediaMode,
    PlaylistRange, VideoCodec,
};

#[derive(Clone, PartialEq)]
pub(super) struct ParsedMedia {
    pub(super) url: String,
    pub(super) mode: MediaMode,
    pub(super) playlist: Option<PlaylistRange>,
    pub(super) clip: Option<ClipRange>,
    pub(super) engine: Option<Engine>,
    pub(super) overrides: DownloadPreferenceOverrides,
}

impl fmt::Debug for ParsedMedia {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedMedia")
            .field("url", &"[REDACTED]")
            .field("mode", &self.mode)
            .field("playlist", &self.playlist)
            .field("clip", &self.clip)
            .field("engine", &self.engine)
            .field("overrides", &self.overrides)
            .finish()
    }
}

pub(super) fn parse(name: &str, args: &[&str]) -> Option<Result<ParsedMedia, ()>> {
    let mode = match name {
        "video" => MediaMode::Video,
        "audio" => MediaMode::Audio,
        "image" => MediaMode::Image,
        "subs" => MediaMode::Subtitles,
        "playlist" | "cut" => MediaMode::Video,
        _ => return None,
    };
    Some(parse_inner(name, args, mode))
}

fn parse_inner(name: &str, args: &[&str], mode: MediaMode) -> Result<ParsedMedia, ()> {
    let (args, overrides) = parse_overrides(name, args)?;
    let (url_index, url) = exactly_one_url(&args)?;
    let url_value = Url::parse(url).map_err(|_| ())?;
    if !matches!(url_value.scheme(), "http" | "https")
        || url_value.host_str().is_none()
        || !url_value.username().is_empty()
        || url_value.password().is_some()
        || url.len() > 4096
    {
        return Err(());
    }
    let tail = args
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != url_index)
        .map(|(_, token)| *token)
        .collect::<Vec<_>>();

    let (playlist, clip, engine) = match name {
        "playlist" => {
            if tail.len() > 1 {
                return Err(());
            }
            let range = match tail.first() {
                None => PlaylistRange::new(1, Some(20)).map_err(|_| ())?,
                Some(token) => playlist_range(token, true)?,
            };
            (Some(range), None, Some(Engine::YtDlp))
        }
        "cut" => {
            if url_index != 0 || tail.len() != 2 {
                return Err(());
            }
            let start = parse_time(tail[0])?;
            let second = parse_time(tail[1])?;
            let end = if !tail[1].contains(':') && second < start {
                start + second
            } else {
                second
            };
            (
                None,
                Some(ClipRange::new(start, end).map_err(|_| ())?),
                None,
            )
        }
        "subs" => {
            if !tail.is_empty() {
                return Err(());
            }
            (None, None, Some(Engine::YtDlp))
        }
        "image" => {
            if !tail.is_empty() {
                return Err(());
            }
            (None, None, Some(Engine::Gallery))
        }
        _ if url_index > 0 => {
            // A prefix is an explicit playlist selector, even if the original
            // source URL contains a timestamp. Inline clip arguments follow URL.
            if url_index != 1 || tail.len() != 1 {
                return Err(());
            }
            (Some(playlist_range(tail[0], false)?), None, None)
        }
        _ => {
            let (playlist, clip) = parse_media_args(url, tail)?;
            (playlist, clip, None)
        }
    };
    Ok(ParsedMedia {
        url: url.to_owned(),
        mode,
        playlist,
        clip,
        engine,
        overrides,
    })
}

fn parse_overrides<'a>(
    name: &str,
    args: &[&'a str],
) -> Result<(Vec<&'a str>, DownloadPreferenceOverrides), ()> {
    let mut positional = Vec::with_capacity(args.len());
    let mut overrides = DownloadPreferenceOverrides::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let token = args[index];
        if !token.starts_with("--") {
            positional.push(token);
            index += 1;
            continue;
        }
        if !seen.insert(token) {
            return Err(());
        }
        let value = *args.get(index + 1).ok_or(())?;
        if value.starts_with("--") {
            return Err(());
        }
        if name == "subs" && !matches!(token, "--subtitle-language" | "--subtitle-auto") {
            return Err(());
        }
        match token {
            "--subtitle-language" => {
                if name != "subs" {
                    return Err(());
                }
                yoink_domain::SubtitleOptions::new(value, false).map_err(|_| ())?;
                overrides.subtitle_language = Some(value.into());
            }
            "--subtitle-auto" => {
                if name != "subs" {
                    return Err(());
                }
                overrides.subtitle_automatic = Some(parse_bool(value)?);
            }
            "--height" => {
                if name == "audio" || name == "image" {
                    return Err(());
                }
                overrides.max_height = Some(if value == "best" {
                    None
                } else {
                    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err(());
                    }
                    let height = value.parse::<u32>().map_err(|_| ())?;
                    if !(1..=100_000).contains(&height) {
                        return Err(());
                    }
                    Some(height)
                });
            }
            "--video-codec" => {
                if name == "audio" || name == "image" {
                    return Err(());
                }
                overrides.video_codec = Some(match value {
                    "h264" => VideoCodec::H264,
                    "h265" => VideoCodec::H265,
                    "av1" => VideoCodec::Av1,
                    "vp9" => VideoCodec::Vp9,
                    "any" => VideoCodec::Any,
                    _ => return Err(()),
                });
            }
            "--audio-codec" => {
                if name == "image" {
                    return Err(());
                }
                overrides.audio_codec = Some(match value {
                    "best" => AudioCodec::Best,
                    "aac" => AudioCodec::Aac,
                    "mp3" => AudioCodec::Mp3,
                    "opus" => AudioCodec::Opus,
                    _ => return Err(()),
                });
            }
            "--container" => {
                if name == "audio" || name == "image" {
                    return Err(());
                }
                overrides.container = Some(match value {
                    "mp4" => Container::Mp4,
                    "mkv" => Container::Mkv,
                    "webm" => Container::Webm,
                    _ => return Err(()),
                });
            }
            "--send-as-file" => overrides.send_as_file = Some(parse_bool(value)?),
            "--gallery-zip" => {
                if name != "image" {
                    return Err(());
                }
                overrides.gallery_zip = Some(parse_bool(value)?);
            }
            _ => return Err(()),
        }
        index += 2;
    }
    Ok((positional, overrides))
}

#[cfg(test)]
#[test]
fn standalone_subtitles_accept_only_their_exact_options() {
    let url = "https://example.test/watch?t=30";
    let parsed = parse(
        "subs",
        &[
            url,
            "--subtitle-language",
            "zh-Hans",
            "--subtitle-auto",
            "true",
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(parsed.mode, MediaMode::Subtitles);
    assert!(parsed.playlist.is_none() && parsed.clip.is_none());
    assert_eq!(
        parsed.overrides.subtitle_language.as_deref(),
        Some("zh-Hans")
    );
    assert_eq!(parsed.overrides.subtitle_automatic, Some(true));
    assert!(parse("subs", &[url]).unwrap().is_ok());
    for args in [
        vec![url, "en"],
        vec![url, "--height", "720"],
        vec![url, "--send-as-file", "true"],
        vec![url, "--subtitle-language", "en.*"],
        vec![url, "--subtitle-auto", "true", "--subtitle-auto", "false"],
        vec![],
    ] {
        assert!(parse("subs", &args).unwrap().is_err());
    }
    assert!(
        parse("video", &[url, "--subtitle-language", "en"])
            .unwrap()
            .is_err()
    );
}

fn parse_bool(value: &str) -> Result<bool, ()> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(()),
    }
}

fn exactly_one_url<'a>(args: &'a [&'a str]) -> Result<(usize, &'a str), ()> {
    let mut urls = args
        .iter()
        .enumerate()
        .filter(|(_, value)| value.starts_with("http://") || value.starts_with("https://"));
    let first = urls.next().ok_or(())?;
    if urls.next().is_some() {
        return Err(());
    }
    Ok((first.0, first.1))
}

fn parse_media_args(
    url: &str,
    tokens: Vec<&str>,
) -> Result<(Option<PlaylistRange>, Option<ClipRange>), ()> {
    if tokens.is_empty() {
        return Ok((None, None));
    }
    if tokens.len() > 2 {
        return Err(());
    }

    let start_hint = query_start(url)?;
    let clip_marker = start_hint.is_some()
        || tokens.iter().any(|token| token.contains(':'))
        || tokens.len() == 2 && tokens.iter().all(|token| is_seconds_syntax(token));
    if clip_marker {
        let (start, end) = match (start_hint, tokens.as_slice()) {
            (Some(start), [end_token]) => (start, inline_end(start, end_token)?),
            (Some(_), _) => return Err(()),
            (None, [start_token, end_token]) => {
                let start = parse_time(start_token)?;
                (start, inline_end(start, end_token)?)
            }
            _ => return Err(()),
        };
        return Ok((None, Some(ClipRange::new(start, end).map_err(|_| ())?)));
    }

    let token = match tokens.as_slice() {
        [one] => *one,
        _ => return Err(()),
    };
    Ok((Some(playlist_range(token, false)?), None))
}

fn inline_end(start: f64, token: &str) -> Result<f64, ()> {
    let value = parse_time(token)?;
    if token.contains(':') {
        Ok(value)
    } else {
        let end = start + value;
        if end.is_finite() { Ok(end) } else { Err(()) }
    }
}

fn playlist_range(token: &str, playlist_command: bool) -> Result<PlaylistRange, ()> {
    let separator = token
        .char_indices()
        .skip(1)
        .find_map(|(index, ch)| (ch == '-').then_some(index));
    let (start_text, end_text) = match separator {
        Some(index) => (&token[..index], Some(&token[index + 1..])),
        None => (token, None),
    };
    let start = start_text.parse::<i32>().map_err(|_| ())?;
    let end = end_text
        .map(|value| value.parse::<i32>().map_err(|_| ()))
        .transpose()?;
    if end_text.is_some_and(str::is_empty) {
        return Err(());
    }
    let (start, end) = match end {
        Some(end) => (start, Some(end)),
        None if start < 0 => (start, None),
        None if playlist_command => (1, Some(start)),
        None => (start, Some(start)),
    };
    PlaylistRange::new(start, end).map_err(|_| ())
}

/// Reads a unique, strictly valid t/start marker without normalizing the original URL.
fn query_start(url: &str) -> Result<Option<f64>, ()> {
    let parsed = Url::parse(url).map_err(|_| ())?;
    let mut marker = None;
    for (key, value) in parsed.query_pairs() {
        if key == "t" || key == "start" {
            if marker.is_some() {
                return Err(());
            }
            marker = Some(parse_start_value(&value)?);
        }
    }
    Ok(marker)
}

/// Accepts unsigned seconds or compact legacy forms such as 1h2m3s and 25m10s.
fn parse_start_value(value: &str) -> Result<f64, ()> {
    if is_seconds_syntax(value) {
        return parse_seconds(value);
    }

    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return Err(());
    }
    let mut index = 0;
    let mut total = 0_u64;
    let mut previous_unit = 4_u8;
    while index < bytes.len() {
        let number_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if number_start == index || index == bytes.len() {
            return Err(());
        }
        let number = value[number_start..index].parse::<u64>().map_err(|_| ())?;
        let (unit, multiplier) = match bytes[index] {
            b'h' => (3, 3600_u64),
            b'm' => (2, 60_u64),
            b's' => (1, 1_u64),
            _ => return Err(()),
        };
        if unit >= previous_unit {
            return Err(());
        }
        previous_unit = unit;
        total = total
            .checked_add(number.checked_mul(multiplier).ok_or(())?)
            .ok_or(())?;
        index += 1;
    }
    Ok(total as f64)
}

fn is_seconds_syntax(value: &str) -> bool {
    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        && parts.next().is_none()
}

fn parse_seconds(value: &str) -> Result<f64, ()> {
    if !is_seconds_syntax(value) {
        return Err(());
    }
    let whole = value
        .split('.')
        .next()
        .ok_or(())?
        .parse::<u64>()
        .map_err(|_| ())?;
    let fractional_part = value.split_once('.').map(|(_, fraction)| fraction);
    if whole == u64::MAX
        && fractional_part.is_some_and(|fraction| fraction.bytes().any(|byte| byte != b'0'))
    {
        return Err(());
    }
    let seconds = value.parse::<f64>().map_err(|_| ())?;
    if seconds.is_finite() && seconds >= 0.0 {
        Ok(seconds)
    } else {
        Err(())
    }
}

pub(super) fn parse_time(value: &str) -> Result<f64, ()> {
    if !value.contains(':') {
        return parse_seconds(value);
    }
    let parts = value.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(());
    }
    let nums = parts
        .iter()
        .map(|part| part.parse::<u64>().map_err(|_| ()))
        .collect::<Result<Vec<_>, _>>()?;
    if nums.iter().skip(1).any(|part| *part >= 60) {
        return Err(());
    }
    let seconds = if nums.len() == 2 {
        nums[0]
            .checked_mul(60)
            .and_then(|value| value.checked_add(nums[1]))
    } else {
        nums[0]
            .checked_mul(3600)
            .and_then(|value| {
                nums[1]
                    .checked_mul(60)
                    .and_then(|minutes| value.checked_add(minutes))
            })
            .and_then(|value| value.checked_add(nums[2]))
    }
    .ok_or(())?;
    Ok(seconds as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://example.test/a";
    fn parsed(name: &str, args: &[&str]) -> ParsedMedia {
        parse(name, args)
            .expect("recognized command")
            .expect("valid media arguments")
    }
    fn rejects(name: &str, args: &[&str]) {
        assert!(
            parse(name, args).expect("recognized command").is_err(),
            "{name} {args:?}"
        );
    }

    #[test]
    fn explicit_ranges_default_caps_and_signed_ranges() {
        for (name, args, expected) in [
            ("playlist", &[URL][..], (1, Some(20))),
            ("playlist", &["3", URL][..], (1, Some(3))),
            ("playlist", &["2-4", URL][..], (2, Some(4))),
            ("audio", &["3", URL][..], (3, Some(3))),
            ("audio", &["-5", URL][..], (-5, None)),
            ("audio", &["-5--1", URL][..], (-5, Some(-1))),
            ("audio", &["5-1", URL][..], (5, Some(1))),
            (
                "audio",
                &["-2147483648--2147483647", URL][..],
                (i32::MIN, Some(i32::MIN + 1)),
            ),
            (
                "audio",
                &["2147483646-2147483647", URL][..],
                (i32::MAX - 1, Some(i32::MAX)),
            ),
            (
                "audio",
                &["2147483647-2147483646", URL][..],
                (i32::MAX, Some(i32::MAX - 1)),
            ),
        ] {
            let media = parsed(name, args);
            if name == "playlist" {
                assert_eq!(media.engine, Some(Engine::YtDlp));
            }
            let range = media.playlist.unwrap();
            assert_eq!((range.start(), range.end()), expected);
        }
        for args in [
            &["0", URL][..],
            &["1-0", URL][..],
            &["-1-2", URL][..],
            &["1--2", URL][..],
            &["1-2147483648", URL][..],
            &["101", URL][..],
            &["-2147483648", URL][..],
            &["1-101", URL][..],
            &["-101", URL][..],
            &["1-", URL][..],
            &["1--", URL][..],
            &["1-2-3", URL][..],
        ] {
            rejects("playlist", args);
        }
        rejects("audio", &["-5--1", URL, "extra"]);
    }

    #[test]
    fn playlist_prefix_is_not_reinterpreted_by_url_timestamps() {
        for mode in ["audio", "video"] {
            for query in ["t=60", "start=1h2m3s", "t=bad", "t=1&t=2"] {
                let url = format!("{URL}?{query}");
                for (selector, expected) in [
                    ("2", (2, Some(2))),
                    ("2-4", (2, Some(4))),
                    ("-2", (-2, None)),
                ] {
                    let media = parsed(mode, &[selector, &url]);
                    let range = media.playlist.unwrap();
                    assert_eq!((range.start(), range.end()), expected);
                    assert_eq!(media.clip, None);
                    assert_eq!(media.url, url);
                }
            }
            rejects(mode, &["2", URL, "3"]);
            rejects(mode, &["2", "3", URL]);
        }
        let clip = parsed("audio", &["https://example.test/a?t=60", "2"]);
        assert_eq!(clip.playlist, None);
        assert_eq!(clip.clip.unwrap().end_seconds(), 62.0);
    }

    #[test]
    fn cut_uses_legacy_second_numeric_token_semantics_and_decimal_seconds() {
        for (start, second, expected_end) in [
            ("0.2", "0.7", 0.7),
            ("10", "5", 15.0),
            ("10", "12", 12.0),
            ("0", "0.25", 0.25),
        ] {
            let clip = parsed("cut", &[URL, start, second]).clip.unwrap();
            assert_eq!(clip.start_seconds(), start.parse::<f64>().unwrap());
            assert_eq!(clip.end_seconds(), expected_end);
        }
        let absolute = parsed("cut", &[URL, "00:01:00", "00:02:00"]).clip.unwrap();
        assert_eq!(absolute.end_seconds(), 120.0);
        for invalid in ["-1", "+1", ".5", "1.", "1e2", "NaN", "inf", "1e999999"] {
            rejects("cut", &[URL, invalid, "10"]);
            rejects("cut", &[URL, "1", invalid]);
        }
        rejects("cut", &[URL, "1:99", "2:00"]);
        rejects("cut", &[URL, "+1:00", "2:00"]);
        rejects("cut", &[URL, "1:00", "2:+00"]);
        rejects("cut", &[URL, "18446744073709551616", "1"]);
        rejects("cut", &[URL, "1", "18446744073709551616"]);
        rejects("cut", &[URL, "10", "0"]);
    }

    #[test]
    fn inline_clips_are_unambiguous_and_query_markers_are_strict() {
        for (args, expected_start, expected_end) in [
            (&[URL, "0.2", "0.7"][..], 0.2, 0.9),
            (&[URL, "1", "1"][..], 1.0, 2.0),
            (&[URL, "00:01:00", "00:02:00"][..], 60.0, 120.0),
            (&["https://example.test/a?t=60", "30"][..], 60.0, 90.0),
            (
                &["https://example.test/a?t=1h2m3s", "25"][..],
                3723.0,
                3748.0,
            ),
            (
                &["https://example.test/a?start=25m10s", "00:30:00"][..],
                1510.0,
                1800.0,
            ),
        ] {
            let clip = parsed("video", args).clip.unwrap();
            assert_eq!(clip.start_seconds(), expected_start);
            assert!((clip.end_seconds() - expected_end).abs() < 1e-9);
        }
        for url in [
            "https://example.test/a?t=bad",
            "https://example.test/a?t=1h2s3m",
            "https://example.test/a?t=1h1h",
            "https://example.test/a?t=18446744073709551616h",
            "https://example.test/a?t=1000000000000000000h",
            "https://example.test/a?t=NaN",
            "https://example.test/a?t=inf",
            "https://example.test/a?t=+1",
            "https://example.test/a?t=1&t=2",
            "https://example.test/a?t=1&start=2",
            "https://example.test/a?start=-1",
            "https://example.test/a?t=1e2",
        ] {
            rejects("video", &[url, "3"]);
            rejects("video", &[url, "1", "3"]);
        }
        rejects("video", &[URL, "3", "4", "5"]);
        rejects(
            "video",
            &["https://example.test/a?t=18446744073709551615h", "1"],
        );
    }

    #[test]
    fn requires_one_safe_url_preserves_bytes_and_redacts_debug() {
        let original = "https://example.test/a?token=secret&x=%2f";
        let media = parsed("video", &[original]);
        assert_eq!(media.url, original);
        let debug = format!("{media:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("example.test"));
        for args in [
            &[][..],
            &["bad"][..],
            &["https://example.test/a", "https://example.test/b"][..],
            &["https://user:pass@example.test/a"][..],
            &["ftp://example.test/a"][..],
        ] {
            rejects("video", args);
        }
        rejects("cut", &["3", URL, "1"]);
        rejects("cut", &[URL, "1"]);
        rejects("image", &[URL, "extra"]);
        rejects("playlist", &[URL, "1", "2"]);
        let overlong = format!("https://example.test/{}", "x".repeat(4080));
        rejects("video", &[&overlong]);
    }

    #[test]
    fn override_flags_are_typed_strict_and_preserve_absence() {
        rejects("playlist", &[URL, "--gallery-zip", "true"]);
        rejects("cut", &[URL, "0", "1", "--gallery-zip", "false"]);
        let media = parsed(
            "video",
            &[
                URL,
                "--height",
                "best",
                "--video-codec",
                "any",
                "--audio-codec",
                "best",
                "--container",
                "mkv",
                "--send-as-file",
                "false",
            ],
        );
        assert_eq!(media.overrides.max_height, Some(None));
        assert_eq!(media.overrides.video_codec, Some(VideoCodec::Any));
        assert_eq!(media.overrides.audio_codec, Some(AudioCodec::Best));
        assert_eq!(media.overrides.container, Some(Container::Mkv));
        assert_eq!(media.overrides.send_as_file, Some(false));
        assert_eq!(media.overrides.gallery_zip, None);
        assert_eq!(
            parsed("video", &[URL]).overrides,
            DownloadPreferenceOverrides::default()
        );
        for value in ["0", "100001", "-1", "+1", "1.5", "BEST"] {
            rejects("video", &[URL, "--height", value]);
        }
        assert_eq!(
            parsed("video", &[URL, "--height", "1"])
                .overrides
                .max_height,
            Some(Some(1))
        );
        assert_eq!(
            parsed("video", &[URL, "--height", "100000"])
                .overrides
                .max_height,
            Some(Some(100000))
        );
        for value in ["h264", "h265", "av1", "vp9", "any"] {
            assert!(
                parsed("video", &[URL, "--video-codec", value])
                    .overrides
                    .video_codec
                    .is_some()
            );
        }
        for value in ["best", "aac", "mp3", "opus"] {
            assert!(
                parsed("video", &[URL, "--audio-codec", value])
                    .overrides
                    .audio_codec
                    .is_some()
            );
        }
        for value in ["mp4", "mkv", "webm"] {
            assert!(
                parsed("video", &[URL, "--container", value])
                    .overrides
                    .container
                    .is_some()
            );
        }
        assert_eq!(
            parsed("video", &[URL, "--send-as-file", "true"])
                .overrides
                .send_as_file,
            Some(true)
        );
        assert_eq!(
            parsed("image", &[URL, "--gallery-zip", "true"])
                .overrides
                .gallery_zip,
            Some(true)
        );
        for args in [
            &[URL, "--video-codec", "bad"][..],
            &[URL, "--audio-codec", "bad"],
            &[URL, "--container", "bad"],
            &[URL, "--send-as-file", "yes"],
            &[URL, "--gallery-zip", "1"],
            &[URL, "--height"],
            &[URL, "--container"],
            &[URL, "--unknown", "x"],
            &[URL, "--height", "1", "--height", "2"],
            &[URL, "--height", "1", "--height"],
            &[URL, "--send-as-file", "false", "--send-as-file", "true"],
        ] {
            rejects("video", args);
        }
        assert_eq!(
            parsed(
                "image",
                &[URL, "--gallery-zip", "false", "--send-as-file", "true"]
            )
            .overrides
            .gallery_zip,
            Some(false)
        );
        assert_eq!(
            parsed("playlist", &["2-4", URL, "--height", "720"])
                .playlist
                .unwrap()
                .start(),
            2
        );
        let cut = parsed("cut", &[URL, "00:01", "00:03", "--video-codec", "h264"]);
        assert_eq!(cut.clip.unwrap().end_seconds(), 3.0);
        for (name, flag) in [
            ("audio", "--height"),
            ("audio", "--video-codec"),
            ("audio", "--container"),
            ("audio", "--gallery-zip"),
            ("image", "--height"),
            ("image", "--video-codec"),
            ("image", "--audio-codec"),
            ("image", "--container"),
            ("video", "--gallery-zip"),
        ] {
            rejects(name, &[URL, flag, "true"]);
        }
    }

    #[test]
    fn media_command_suffixes_and_unknown_names_are_handled() {
        assert!(parse("not-a-command", &[URL]).is_none());
        assert!(parse("image", &[URL]).unwrap().is_ok());
        assert_eq!(parsed("image", &[URL]).engine, Some(Engine::Gallery));
        assert!(parse("video", &[URL, "extra", "extra"]).unwrap().is_err());
    }
}
