use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Russian,
}

impl Language {
    pub fn from_code(code: Option<&str>) -> Self {
        if code.is_some_and(|code| {
            let mut parts = code.split('-');
            let base_matches = parts
                .next()
                .is_some_and(|base| base.eq_ignore_ascii_case("ru"));
            let valid_subtag = match parts.next() {
                None => true,
                Some(subtag) => !subtag.is_empty() && parts.next().is_none(),
            };
            base_matches && valid_subtag
        }) {
            Self::Russian
        } else {
            Self::English
        }
    }
}

macro_rules! define_keys {
    ($( $variant:ident => $name:literal $(, [$($parameter:literal),*])?; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Key { $( $variant, )* }

        impl Key {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $name,)* }
            }
            fn placeholders(self) -> &'static [&'static str] {
                match self { $(Self::$variant => &[$($($parameter,)*)?],)* }
            }
        }
    }
}

define_keys! {
    ProxyUsage => "telegram.proxy_usage";
    ProxyTitle => "telegram.proxy_title", ["status"];
    ProxyEnabled => "telegram.proxy_enabled";
    ProxyDisabled => "telegram.proxy_disabled";
    ProxyEnable => "telegram.proxy_enable";
    ProxyDisable => "telegram.proxy_disable";
    ProxyUnavailable => "telegram.proxy_unavailable";
    ProxyStale => "telegram.proxy_stale";
    SettingsStale => "telegram.settings_stale";
    SubsTitle => "telegram.subs_title", ["values"];
    SubsAutomaticOn => "telegram.subs_automatic_on";
    SubsAutomaticOff => "telegram.subs_automatic_off";
    SubsInvalidLanguage => "telegram.subs_invalid_language";
    SubsStale => "telegram.subs_stale";
    CutUsage => "telegram.cut_usage";
    CutAskStart => "telegram.cut_ask_start";
    CutAskEnd => "telegram.cut_ask_end";
    CutSummary => "telegram.cut_summary", ["start", "end"];
    CutRanges => "telegram.cut_ranges", ["ranges"];
    CutAdd => "telegram.cut_add";
    CutDownload => "telegram.cut_download";
    CutChangeSegment => "telegram.cut_change_segment", ["index"];
    CutDeleteSegment => "telegram.cut_delete_segment", ["index"];
    CutBack => "telegram.cut_back";
    CutConfirming => "telegram.cut_confirming";
    CutQualityTitle => "telegram.cut_quality_title";
    CutSubmitted => "telegram.cut_submitted", ["id"];
    CutCancelled => "telegram.cut_cancelled";
    CutTimeHint => "telegram.cut_time_hint";
    CutInvalidTime => "telegram.cut_invalid_time";
    CutStale => "telegram.cut_stale";
    CutRetry => "telegram.cut_retry";
    CutNothingToCancel => "telegram.cut_nothing_to_cancel";
    CutGo => "telegram.cut_go";
    CutEdit => "telegram.cut_edit";
    CutCancel => "telegram.cut_cancel";
    QualityUsage => "telegram.quality_usage";
    QualityTitle => "telegram.quality_title";
    QualityBest => "telegram.quality_best";
    QualityAudio => "telegram.quality_audio";
    QualityHeight => "telegram.quality_height", ["height"];
    QualityCancel => "telegram.quality_cancel";
    QualityConfirming => "telegram.quality_confirming", ["choice"];
    QualitySubmitted => "telegram.quality_submitted", ["id"];
    QualityCancelled => "telegram.quality_cancelled";
    QualityStale => "telegram.quality_stale";
    QualityRetry => "telegram.quality_retry";
    QualityCut => "telegram.quality_cut";
    QualityCutting => "telegram.quality_cutting";
    QualityResumeCut => "telegram.quality_resume_cut";
    QualityCutStale => "telegram.quality_cut_stale";
    QualityCutActive => "telegram.quality_cut_active";
    SettingsUsage => "telegram.settings_usage";
    SettingsTitle => "telegram.settings_title", ["values"];
    SettingsQuality => "telegram.settings_quality";
    SettingsCodec => "telegram.settings_codec";
    SettingsContainer => "telegram.settings_container";
    SettingsAudioCodec => "telegram.settings_audio_codec";
    SettingsSendAsFile => "telegram.settings_send_as_file";
    SettingsGalleryZip => "telegram.settings_gallery_zip";
    SettingsSubtitleLanguage => "telegram.settings_subtitle_language";
    SettingsSubtitleAutomatic => "telegram.settings_subtitle_automatic";
    SettingsOn => "telegram.settings_on";
    SettingsOff => "telegram.settings_off";
    SettingsBest => "telegram.settings_best";
    SettingsAsk => "telegram.settings_ask";
    SettingsBack => "telegram.settings_back";
    LangUsage => "telegram.lang_usage";
    LangChoose => "telegram.lang_choose";
    LangEnglish => "telegram.lang_english";
    LangRussian => "telegram.lang_russian";
    LangSet => "telegram.lang_set", ["language"];
    LangStale => "telegram.lang_stale";
    HelpEnabled => "telegram.help_enabled";
    HelpBasic => "telegram.help_basic";
    UserHelpEnabled => "telegram.user_help_enabled";
    UserHelpBasic => "telegram.user_help_basic";
    RegistrationPending => "telegram.registration_pending";
    UsageDownload => "telegram.usage_download";
    PlainUrlUsage => "telegram.plain_url_usage";
    UncacheUsage => "telegram.uncache_usage";
    UncacheDone => "telegram.uncache_done", ["count"];
    UncacheEmpty => "telegram.uncache_empty";
    UncacheStale => "telegram.uncache_stale";
    CookieUsage => "telegram.cookie_usage";
    CookieList => "telegram.cookie_list", ["domains"];
    CookieEmpty => "telegram.cookie_empty";
    CookieNext => "telegram.cookie_next";
    CookieInvalidEntry => "telegram.cookie_invalid_entry";
    CookieStored => "telegram.cookie_stored", ["domain", "count"];
    CookieMissing => "telegram.cookie_missing", ["domain"];
    CookieInvalid => "telegram.cookie_invalid", ["domain"];
    CookieRemoved => "telegram.cookie_removed", ["count"];
    CookieProcessed => "telegram.cookie_processed";
    CookieStale => "telegram.cookie_stale";
    CookieUploadGuidance => "telegram.cookie_upload_guidance";
    CookieUploadRetry => "telegram.cookie_upload_retry";
    CookieUploadReplay => "telegram.cookie_upload_replay";
    CookieUrlUsage => "telegram.cookie_url_usage";
    CookieUrlFetchError => "telegram.cookie_url_fetch_error";
    CookieUrlSaved => "telegram.cookie_url_saved";
    DownloadsUnavailable => "telegram.downloads_unavailable";
    Status => "telegram.status", ["version", "readiness", "modules"];
    ReadinessReady => "telegram.readiness_ready";
    ReadinessNotReady => "telegram.readiness_not_ready";
    ErrorDenied => "telegram.error_denied";
    ErrorCapacity => "telegram.error_capacity";
    ErrorConflict => "telegram.error_conflict";
    ErrorInvalidInput => "telegram.error_invalid_input";
    ErrorInvalidPreferences => "telegram.error_invalid_preferences";
    ErrorInvalidCredentials => "telegram.error_invalid_credentials";
    ErrorProxyUnavailable => "telegram.error_proxy_unavailable";
    ErrorUnsupportedAuthentication => "telegram.error_unsupported_authentication";
    ErrorChangedCredentials => "telegram.error_changed_credentials";
    ErrorInvalidConfig => "telegram.error_invalid_config";
    ErrorOperation => "telegram.error_operation";
    StatusQueued => "telegram.status_queued";
    StatusRunning => "telegram.status_running";
    StatusDelivering => "telegram.status_delivering";
    StatusDeliveryUnknown => "telegram.status_delivery_unknown";
    StatusDeliveryPartial => "telegram.status_delivery_partial";
    StatusSucceeded => "telegram.status_succeeded";
    StatusFailed => "telegram.status_failed";
    StatusCancelled => "telegram.status_cancelled";
    Operation => "telegram.operation", ["id", "status", "attempt"];
    DeliveryUnconfirmed => "telegram.delivery_unconfirmed";
    ProgressFetching => "telegram.progress_fetching";
    ProgressProcessing => "telegram.progress_processing";
    ProgressWriting => "telegram.progress_writing";
    ProgressPercent => "telegram.progress_percent", ["percent"];
    ProgressDelivered => "telegram.progress_delivered", ["delivered", "total"];
    DeliveryPartialHint => "telegram.delivery_partial_hint";
    DeliveredMessage => "telegram.delivered_message", ["id"];
    ExistingOperation => "telegram.existing_operation", ["id"];
    OperationCreated => "telegram.operation_created", ["id"];
    CancellationRequested => "telegram.cancellation_requested", ["id"];
    OperationNotFound => "telegram.operation_not_found";
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    Parse {
        locale: &'static str,
        message: String,
    },
    InvalidShape {
        locale: &'static str,
        key: String,
    },
    NonString {
        locale: &'static str,
        key: String,
    },
    EmptyTranslation {
        locale: &'static str,
        key: String,
    },
    MissingKey {
        locale: &'static str,
        key: &'static str,
    },
    ExtraKey {
        locale: &'static str,
        key: String,
    },
    MalformedTemplate {
        locale: &'static str,
        key: &'static str,
    },
    PlaceholderMismatch {
        locale: &'static str,
        key: &'static str,
        parameter: String,
    },
    MissingParameter {
        locale: &'static str,
        key: &'static str,
        parameter: String,
    },
    ExtraParameter {
        locale: &'static str,
        key: &'static str,
        parameter: String,
    },
    DuplicateParameter {
        locale: &'static str,
        key: &'static str,
        parameter: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Parameter(&'static str),
}
type CompiledTemplate = Vec<Segment>;
type LocaleCatalog = BTreeMap<Key, CompiledTemplate>;

pub struct Catalog {
    locales: [LocaleCatalog; 2],
}
static EMBEDDED: OnceLock<Result<Catalog, CatalogError>> = OnceLock::new();

pub fn embedded() -> Result<&'static Catalog, &'static CatalogError> {
    EMBEDDED.get_or_init(Catalog::embedded).as_ref()
}

impl Catalog {
    pub fn embedded() -> Result<Self, CatalogError> {
        let en = parse(include_str!("../locales/en.toml"), "en")?;
        let ru = parse(include_str!("../locales/ru.toml"), "ru")?;
        validate(&en, &ru)?;
        Ok(Self { locales: [en, ru] })
    }

    pub fn render(
        &self,
        language: Language,
        key: Key,
        parameters: &[(&str, &str)],
    ) -> Result<String, CatalogError> {
        let locale_name = match language {
            Language::English => "en",
            Language::Russian => "ru",
        };
        let expected = key.placeholders();
        let mut values = BTreeMap::new();
        for &(name, value) in parameters {
            if !expected.contains(&name) {
                return Err(CatalogError::ExtraParameter {
                    locale: locale_name,
                    key: key.as_str(),
                    parameter: name.to_owned(),
                });
            }
            if values.insert(name, value).is_some() {
                return Err(CatalogError::DuplicateParameter {
                    locale: locale_name,
                    key: key.as_str(),
                    parameter: name.to_owned(),
                });
            }
        }
        for &name in expected {
            if !values.contains_key(name) {
                return Err(CatalogError::MissingParameter {
                    locale: locale_name,
                    key: key.as_str(),
                    parameter: name.to_owned(),
                });
            }
        }
        let locale = match language {
            Language::English => &self.locales[0],
            Language::Russian => &self.locales[1],
        };
        let template = locale
            .get(&key)
            .expect("validated catalog has every manifest key");
        let capacity = template
            .iter()
            .map(|segment| match segment {
                Segment::Literal(text) => text.len(),
                Segment::Parameter(name) => values.get(*name).map_or(0, |value| value.len()),
            })
            .sum();
        let mut output = String::with_capacity(capacity);
        for segment in template {
            match segment {
                Segment::Literal(text) => output.push_str(text),
                Segment::Parameter(name) => output.push_str(values[*name]),
            }
        }
        Ok(output)
    }
}

fn parse(source: &str, locale: &'static str) -> Result<LocaleCatalog, CatalogError> {
    let value: toml::Value = toml::from_str(source).map_err(|error| CatalogError::Parse {
        locale,
        message: error.to_string(),
    })?;
    let root = value.as_table().ok_or_else(|| CatalogError::InvalidShape {
        locale,
        key: "<root>".into(),
    })?;
    if root.len() != 1 || !root.contains_key("telegram") {
        return Err(CatalogError::InvalidShape {
            locale,
            key: "<root>".into(),
        });
    }
    let table = root["telegram"]
        .as_table()
        .ok_or_else(|| CatalogError::InvalidShape {
            locale,
            key: "telegram".into(),
        })?;
    let mut entries = BTreeMap::new();
    for (name, value) in table {
        let full_key = format!("telegram.{name}");
        if value.is_table() {
            return Err(CatalogError::InvalidShape {
                locale,
                key: full_key,
            });
        }
        let text = value.as_str().ok_or_else(|| CatalogError::NonString {
            locale,
            key: full_key.clone(),
        })?;
        if text.trim().is_empty() {
            return Err(CatalogError::EmptyTranslation {
                locale,
                key: full_key,
            });
        }
        entries.insert(full_key, text.to_owned());
    }
    let mut result = BTreeMap::new();
    for &key in Key::ALL {
        let name = key.as_str();
        let text = entries
            .remove(name)
            .ok_or(CatalogError::MissingKey { locale, key: name })?;
        let segments = compile_template(&text, locale, name)?;
        let found = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Parameter(param) => Some(*param),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        for &parameter in key.placeholders() {
            if !found.contains(parameter) {
                return Err(CatalogError::PlaceholderMismatch {
                    locale,
                    key: name,
                    parameter: parameter.to_owned(),
                });
            }
        }
        if let Some(parameter) = found
            .iter()
            .find(|parameter| !key.placeholders().contains(parameter))
        {
            return Err(CatalogError::PlaceholderMismatch {
                locale,
                key: name,
                parameter: (*parameter).to_string(),
            });
        }
        result.insert(key, segments);
    }
    if let Some((key, _)) = entries.into_iter().next() {
        return Err(CatalogError::ExtraKey { locale, key });
    }
    Ok(result)
}

fn compile_template(
    template: &str,
    locale: &'static str,
    key: &'static str,
) -> Result<CompiledTemplate, CatalogError> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut chars = template.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' => {
                if !literal.is_empty() {
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                }
                let mut name = String::new();
                let mut closed = false;
                for (_, next) in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    if !(next.is_ascii_alphanumeric() || next == '_') {
                        return Err(CatalogError::MalformedTemplate { locale, key });
                    }
                    name.push(next);
                }
                if !closed || name.is_empty() {
                    return Err(CatalogError::MalformedTemplate { locale, key });
                }
                let parameter = Key::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.as_str() == key)
                    .and_then(|candidate| {
                        candidate
                            .placeholders()
                            .iter()
                            .copied()
                            .find(|expected| *expected == name.as_str())
                    })
                    .ok_or_else(|| CatalogError::PlaceholderMismatch {
                        locale,
                        key,
                        parameter: name.clone(),
                    })?;
                segments.push(Segment::Parameter(parameter));
            }
            '}' => return Err(CatalogError::MalformedTemplate { locale, key }),
            _ => literal.push(ch),
        }
    }
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

fn validate(en: &LocaleCatalog, ru: &LocaleCatalog) -> Result<(), CatalogError> {
    for &key in Key::ALL {
        if !en.contains_key(&key) {
            return Err(CatalogError::MissingKey {
                locale: "en",
                key: key.as_str(),
            });
        }
        if !ru.contains_key(&key) {
            return Err(CatalogError::MissingKey {
                locale: "ru",
                key: key.as_str(),
            });
        }
    }
    Ok(())
}

pub fn language_for(code: Option<&str>) -> Language {
    Language::from_code(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(locale: &str) -> String {
        let prefix = if locale == "en" {
            include_str!("../locales/en.toml")
        } else {
            include_str!("../locales/ru.toml")
        };
        prefix.to_owned()
    }

    #[test]
    fn every_manifest_key_renders_in_both_locales() {
        let catalog = Catalog::embedded().unwrap();
        for key in Key::ALL {
            let params = key
                .placeholders()
                .iter()
                .map(|name| (*name, "x"))
                .collect::<Vec<_>>();
            for language in [Language::English, Language::Russian] {
                assert!(
                    !catalog.render(language, *key, &params).unwrap().is_empty(),
                    "{}",
                    key.as_str()
                );
            }
        }
    }

    #[test]
    fn quality_choice_copy_and_resume_guidance_exist_in_both_locales() {
        let catalog = Catalog::embedded().unwrap();
        for language in [Language::English, Language::Russian] {
            let title = catalog.render(language, Key::QualityTitle, &[]).unwrap();
            let settings = catalog.render(language, Key::SettingsUsage, &[]).unwrap();
            let help = catalog.render(language, Key::HelpEnabled, &[]).unwrap();
            let usage = catalog.render(language, Key::UsageDownload, &[]).unwrap();
            assert!(title.contains(if language == Language::English {
                "not verified source formats"
            } else {
                "наличие форматов у источника не проверяется"
            }));
            assert!(title.contains(if language == Language::English {
                "Best applies no height limit"
            } else {
                "«Лучшее» не ограничивает высоту"
            }));
            assert!(
                catalog
                    .render(language, Key::QualityAudio, &[])
                    .unwrap()
                    .contains(if language == Language::English {
                        "Audio"
                    } else {
                        "аудио"
                    })
            );
            assert!(settings.contains("ask"));
            assert!(help.contains("/quality"));
            assert!(help.contains("/list URL"));
            assert!(usage.contains("/list URL"));
            assert!(help.contains(if language == Language::English {
                "without downloading media"
            } else {
                "без загрузки медиа"
            }));
        }
    }

    #[test]
    fn renders_inserted_braces_unicode_in_a_single_pass() {
        let catalog = Catalog::embedded().unwrap();
        let text = catalog
            .render(
                Language::English,
                Key::Operation,
                &[("id", "{attempt} 世界"), ("status", "x"), ("attempt", "3")],
            )
            .unwrap();
        assert!(text.contains("{attempt} 世界: x"));
    }

    #[test]
    fn validates_parameter_sets_and_does_not_report_values() {
        let catalog = Catalog::embedded().unwrap();
        assert!(
            matches!(catalog.render(Language::English, Key::Operation, &[("id", "secret")]), Err(CatalogError::MissingParameter { parameter, .. }) if parameter == "status")
        );
        assert!(
            matches!(catalog.render(Language::English, Key::Operation, &[("id", "x"), ("id", "secret"), ("status", "x"), ("attempt", "x")]), Err(CatalogError::DuplicateParameter { parameter, .. }) if parameter == "id")
        );
        assert!(
            matches!(catalog.render(Language::English, Key::Operation, &[("id", "x"), ("status", "x"), ("attempt", "x"), ("secret-name", "SECRET VALUE")]), Err(CatalogError::ExtraParameter { parameter, .. }) if parameter == "secret-name")
        );
    }

    #[test]
    fn validates_toml_shape_keys_values_empty_text_and_templates() {
        assert!(matches!(
            parse("[bad]\nx = \"x\"\n", "en"),
            Err(CatalogError::InvalidShape { .. })
        ));
        assert!(matches!(
            parse("[telegram]\nhelp_enabled = 3\n", "en"),
            Err(CatalogError::NonString { .. })
        ));
        assert!(matches!(
            parse("[telegram]\nhelp_enabled = \"\"\n", "en"),
            Err(CatalogError::EmptyTranslation { .. })
        ));
        let mut text = source("en");
        text.push_str("\n[telegram.extra]\nx = \"x\"\n");
        assert!(matches!(
            parse(&text, "en"),
            Err(CatalogError::InvalidShape { .. })
        ));
        let text = source("en").replace("Operation {id}:", "Operation {{id}:");
        assert!(matches!(
            parse(&text, "en"),
            Err(CatalogError::MalformedTemplate { .. })
        ));
        let text = source("en").replace("Operation {id}:", "Operation {other}:");
        assert!(matches!(
            parse(&text, "en"),
            Err(CatalogError::PlaceholderMismatch { .. })
        ));
    }

    #[test]
    fn locale_codes_match_only_supported_base_language() {
        for code in ["ru", "RU", "ru-RU", "RU-ru"] {
            assert_eq!(language_for(Some(code)), Language::Russian);
        }
        for code in ["rubbish", "ru_RU", "ru--RU", "en-US", "fr"] {
            assert_eq!(language_for(Some(code)), Language::English);
        }
        assert_eq!(language_for(None), Language::English);
    }

    #[test]
    fn duplicate_toml_keys_fail_and_missing_or_extra_manifest_keys_are_specific() {
        assert!(matches!(
            parse("[telegram]\na = \"x\"\na = \"y\"\n", "en"),
            Err(CatalogError::Parse { locale: "en", .. })
        ));
        let mut text = source("en").replace("operation_not_found = \"Operation not found.\"\n", "");
        assert!(matches!(
            parse(&text, "en"),
            Err(CatalogError::MissingKey {
                locale: "en",
                key: "telegram.operation_not_found"
            })
        ));
        text = source("en");
        text.push_str("extra = \"x\"\n");
        assert!(matches!(
            parse(&text, "en"),
            Err(CatalogError::ExtraKey { .. })
        ));
    }

    #[test]
    fn embedded_english_and_russian_content_is_preserved() {
        let catalog = Catalog::embedded().unwrap();
        assert_eq!(
            catalog
                .render(Language::English, Key::ErrorProxyUnavailable, &[])
                .unwrap(),
            "Proxy is enabled, but no proxy is configured. Configure one or disable proxy use."
        );
        assert_eq!(
            catalog
                .render(Language::Russian, Key::ErrorProxyUnavailable, &[])
                .unwrap(),
            "Прокси включён, но не настроен. Укажите прокси или отключите его использование."
        );
    }
}
