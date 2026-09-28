use super::*;
use serde_json::{Value, json};

fn message(text: &str, url: &str) -> Value {
    let start = text.find(url).unwrap();
    json!({
        "message_id": 11, "date": 1_700_000_000,
        "chat": {"id": 42, "type": "private", "first_name": "Owner"},
        "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
        "text": text,
        "entities": [{"type":"url", "offset":text[..start].encode_utf16().count(), "length":url.encode_utf16().count()}]
    })
}
fn parse_value(value: Value) -> Option<Result<Action, ()>> {
    parse(&serde_json::from_value(value).unwrap(), 123)
}
fn media(text: &str, url: &str) -> ParsedMedia {
    match parse_value(message(text, url)) {
        Some(Ok(Action::Download(media))) => media,
        result => panic!("expected download, got {result:?}"),
    }
}

#[test]
fn visible_url_uses_utf16_entities_and_redacts_debug() {
    let url = "https://example.test/видео?token=secret-marker";
    let parsed = media(&format!("🦀 Привет {url}"), url);
    assert_eq!(parsed.url, url);
    assert_eq!(parsed.mode, MediaMode::Video);
    assert_eq!(parsed.clip, None);
    assert_eq!(parsed.playlist, None);
    assert_eq!(parsed.overrides, DownloadPreferenceOverrides::default());
    assert!(!format!("{parsed:?}").contains("secret-marker"));
    assert_eq!(
        media("HTTPS://example.test/video", "HTTPS://example.test/video").url,
        "HTTPS://example.test/video"
    );
    let mut value = message(&format!("🦀 {url}"), url);
    value["entities"][0]["offset"] = json!(1); // inside a surrogate pair
    assert_eq!(parse_value(value), Some(Err(())));
    let mut value = message(url, url);
    value["entities"][0]["length"] = json!(usize::MAX);
    assert_eq!(parse_value(value.clone()), Some(Err(())));
    value["entities"][0]["offset"] = json!(1);
    assert_eq!(parse_value(value), Some(Err(())));
}

#[test]
fn clips_are_durations_or_absolute_colon_times_never_playlist_selectors() {
    let url = "https://example.test/video";
    for (text, source, start, end) in [
        (format!("{url} 10 20"), url.to_owned(), 10.0, 30.0),
        (format!("{url} 10 00:20"), url.to_owned(), 10.0, 20.0),
        (format!("{url}?t=10 20"), format!("{url}?t=10"), 10.0, 30.0),
        (format!("{url}?t=10 1 2"), format!("{url}?t=10"), 1.0, 3.0),
    ] {
        let parsed = media(&text, &source);
        assert_eq!(parsed.clip, Some(ClipRange::new(start, end).unwrap()));
        assert_eq!(parsed.playlist, None);
    }
    for suffix in [
        "10",
        "1-2",
        "1 0",
        "10 00:01",
        "1 2 3",
        "1 NaN",
        "--height best",
    ] {
        assert_eq!(
            parse_value(message(&format!("{url} {suffix}"), url)),
            Some(Err(()))
        );
    }
    for source in [
        "https://example.test/?t=bad",
        "https://example.test/?t=1&start=2",
        "https://name:password@example.test/",
    ] {
        assert_eq!(parse_value(message(source, source)), Some(Err(())));
    }
    let source = format!("{url}?t=1m2s");
    let result = parse_value(message(&source, &source));
    assert!(matches!(result, Some(Ok(Action::AskEnd {start, ..})) if start == 62.0));
}

#[test]
fn commands_hidden_links_captions_and_music_are_not_implicit_downloads() {
    let url = "https://example.test/video";
    for prefix in ["/help ", "/unknown ", " /video@other_bot "] {
        assert_eq!(parse_value(message(&format!("{prefix}{url}"), url)), None);
    }
    let mut hidden = message(url, url);
    hidden["entities"] = json!([{"type":"text_link", "offset":0, "length":4,"url":url}]);
    assert_eq!(parse_value(hidden), None);
    let mut missing = message(url, url);
    missing["entities"] = json!([]);
    assert_eq!(parse_value(missing), None);
    let mut caption = message(url, url);
    caption.as_object_mut().unwrap().remove("text");
    caption["caption"] = json!(url);
    caption["photo"] = json!([{"file_id":"x","file_unique_id":"y","width":1,"height":1}]);
    assert_eq!(parse_value(caption), None);
    let mut multiple = message(&format!("{url} {url}"), url);
    multiple["entities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"url","offset":url.len()+1,"length":url.len()}));
    assert_eq!(parse_value(multiple), Some(Err(())));
    let mut foreign = message(url, url);
    foreign["via_bot"] = json!({"id":999,"is_bot":true,"first_name":"Foreign"});
    assert_eq!(parse_value(foreign.clone()), None);
    foreign["via_bot"]["id"] = json!(123);
    foreign["via_bot"]["is_bot"] = json!(false);
    assert_eq!(parse_value(foreign), None);
    let mut command = message(&format!("hey /foo {url}"), url);
    command["entities"].as_array_mut().unwrap().push(json!({
        "type": "bot_command", "offset": 4, "length": 4
    }));
    assert_eq!(parse_value(command), None);
    for host in [
        "open.spotify.com",
        "music.youtube.com",
        "music.yandex.ru",
        "artist.bandcamp.com",
        "soundcloud.com",
    ] {
        let url = format!("https://{host}/track/test");
        assert_eq!(parse_value(message(&url, &url)), None);
    }
    assert!(reserved_music_host("open.spotify.com."));
    assert!(!reserved_music_host("notspotify.com"));
    assert!(!reserved_music_host("spotify.com.example.test"));
    assert!(!reserved_music_host("youtube.com"));
}

#[test]
fn group_trigger_requires_this_bot_reply_or_inline_origin() {
    let url = "https://example.test/video";
    let mut group = message(url, url);
    group["chat"] = json!({"id":-42,"type":"supergroup","title":"Group"});
    assert_eq!(parse_value(group.clone()), None);
    let mut reply = message("prompt", "prompt");
    reply["from"] = json!({"id":999,"is_bot":true,"first_name":"Other"});
    group["reply_to_message"] = reply.clone();
    assert_eq!(parse_value(group.clone()), None);
    reply["from"]["id"] = json!(123);
    group["reply_to_message"] = reply;
    assert!(matches!(
        parse_value(group.clone()),
        Some(Ok(Action::Download(_)))
    ));
    group["via_bot"] = json!({"id":999,"is_bot":true,"first_name":"Other"});
    assert_eq!(parse_value(group.clone()), None);
    group.as_object_mut().unwrap().remove("reply_to_message");
    group["via_bot"]["id"] = json!(123);
    assert!(matches!(parse_value(group), Some(Ok(Action::Download(_)))));
    let source = format!("{url}?t=10");
    let mut group = message(&source, &source);
    group["chat"] = json!({"id":-42,"type":"supergroup","title":"Group"});
    group["via_bot"] = json!({"id":123,"is_bot":true,"first_name":"This"});
    assert_eq!(parse_value(group), Some(Err(())));
}
