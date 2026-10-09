use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloaderSettings {
    pub download_retries: u32,
    pub download_timeout: u64,
    pub max_file_size_mb: u64,
    pub rate_limit_per_minute: u32,
    pub rate_limit_per_hour: u32,
    pub rate_limit_per_day: u32,
    pub max_playlist_count: usize,
}

impl Default for DownloaderSettings {
    fn default() -> Self {
        Self {
            download_retries: 3,
            download_timeout: 1200,
            max_file_size_mb: 2048,
            rate_limit_per_minute: 5,
            rate_limit_per_hour: 30,
            rate_limit_per_day: 100,
            max_playlist_count: 50,
        }
    }
}

impl DownloaderSettings {
    pub fn validate(self) -> Result<Self, &'static str> {
        if !(1..=10).contains(&self.download_retries)
            || !(60..=86400).contains(&self.download_timeout)
            || !(1..=2048).contains(&self.max_file_size_mb)
            || !(1..=500).contains(&self.max_playlist_count)
            || [
                self.rate_limit_per_minute,
                self.rate_limit_per_hour,
                self.rate_limit_per_day,
            ]
            .iter()
            .any(|v| !(1..=1_000_000).contains(v))
        {
            return Err("Invalid downloader limits");
        }
        Ok(self)
    }

    pub fn from_settings(values: &BTreeMap<String, String>) -> Result<Self, &'static str> {
        let mut settings = Self::default();
        macro_rules! read {
            ($field:ident) => {
                if let Some(value) = values.get(concat!("dl.", stringify!($field))) {
                    settings.$field = value.parse().map_err(|_| "Invalid downloader limits")?;
                }
            };
        }
        read!(download_retries);
        read!(download_timeout);
        read!(max_file_size_mb);
        read!(rate_limit_per_minute);
        read!(rate_limit_per_hour);
        read!(rate_limit_per_day);
        read!(max_playlist_count);
        if !values.contains_key("dl.max_file_size_mb")
            && let Some(gb) = values.get("dl.max_file_size_gb")
        {
            let mb = gb.parse::<f64>().map_err(|_| "Invalid downloader limits")? * 1024.0;
            if !mb.is_finite() || !(1.0..=2048.0).contains(&mb) {
                return Err("Invalid downloader limits");
            }
            settings.max_file_size_mb = mb as u64;
        }
        settings.validate()
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloaderSettingsPatch {
    pub download_retries: Option<u32>,
    pub download_timeout: Option<u64>,
    pub max_file_size_mb: Option<u64>,
    pub rate_limit_per_minute: Option<u32>,
    pub rate_limit_per_hour: Option<u32>,
    pub rate_limit_per_day: Option<u32>,
    pub max_playlist_count: Option<usize>,
}
impl DownloaderSettingsPatch {
    pub fn values(&self) -> BTreeMap<String, String> {
        let mut values = BTreeMap::new();
        macro_rules! insert {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    values.insert(concat!("dl.", stringify!($field)).into(), value.to_string());
                }
            };
        }
        insert!(download_retries);
        insert!(download_timeout);
        insert!(max_file_size_mb);
        insert!(rate_limit_per_minute);
        insert!(rate_limit_per_hour);
        insert!(rate_limit_per_day);
        insert!(max_playlist_count);
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_gigabytes_and_patch_validation() {
        let mut values = BTreeMap::from([("dl.max_file_size_gb".into(), "1.5".into())]);
        assert_eq!(
            DownloaderSettings::from_settings(&values)
                .unwrap()
                .max_file_size_mb,
            1536
        );
        values.insert("dl.max_file_size_mb".into(), "1024".into());
        assert_eq!(
            DownloaderSettings::from_settings(&values)
                .unwrap()
                .max_file_size_mb,
            1024
        );
        for value in ["0", "-1", "2049", "NaN"] {
            values.insert("dl.max_file_size_mb".into(), value.into());
            assert!(DownloaderSettings::from_settings(&values).is_err());
        }
    }
}
