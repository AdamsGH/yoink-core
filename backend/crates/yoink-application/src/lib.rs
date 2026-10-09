//! Application services shared by transports. No framework or database types.
pub mod metrics;

use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleStatus {
    Planned,
    Experimental,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleDescriptor {
    pub id: &'static str,
    pub status: ModuleStatus,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemSnapshot {
    pub version: String,
    /// Readiness of configured diagnostic adapters, not product feature parity.
    pub ready: bool,
    pub modules: Vec<ModuleDescriptor>,
}

/// A typed shared dependency. The composition root owns readiness transitions.
#[derive(Clone, Debug)]
pub struct SystemService {
    version: Arc<str>,
    ready: Arc<AtomicBool>,
    downloads_enabled: Arc<AtomicBool>,
    insight_enabled: Arc<AtomicBool>,
    music_enabled: Arc<AtomicBool>,
    stats_enabled: Arc<AtomicBool>,
    inbox_enabled: Arc<AtomicBool>,
    started: Arc<Instant>,
    metrics: metrics::Metrics,
    bot_running: Arc<AtomicBool>,
    bot_username: Arc<RwLock<Option<String>>>,
}

impl SystemService {
    pub fn new(version: impl Into<String>) -> Self {
        let started = Arc::new(Instant::now());
        Self {
            version: Arc::from(version.into()),
            ready: Arc::new(AtomicBool::new(false)),
            downloads_enabled: Arc::new(AtomicBool::new(false)),
            insight_enabled: Arc::new(AtomicBool::new(false)),
            music_enabled: Arc::new(AtomicBool::new(false)),
            stats_enabled: Arc::new(AtomicBool::new(false)),
            inbox_enabled: Arc::new(AtomicBool::new(false)),
            metrics: metrics::Metrics::new(started.clone()),
            started,
            bot_running: Arc::new(AtomicBool::new(false)),
            bot_username: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }

    pub fn uptime_seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
    pub fn metrics(&self) -> metrics::Metrics {
        self.metrics.clone()
    }

    pub fn set_bot_status(&self, running: bool, username: Option<String>) {
        self.bot_running.store(running, Ordering::Release);
        if let Ok(mut current) = self.bot_username.write() {
            *current = username;
        }
    }

    pub fn bot_running(&self) -> bool {
        self.bot_running.load(Ordering::Acquire)
    }

    pub fn bot_username(&self) -> Option<String> {
        self.bot_username
            .read()
            .ok()
            .and_then(|value| value.clone())
    }

    /// Advertises only the implemented progressive-video slice, not download parity.
    pub fn set_downloads_enabled(&self, enabled: bool) {
        self.downloads_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_insight_enabled(&self, enabled: bool) {
        self.insight_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_music_enabled(&self, enabled: bool) {
        self.music_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_stats_enabled(&self, enabled: bool) {
        self.stats_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_inbox_enabled(&self, enabled: bool) {
        self.inbox_enabled.store(enabled, Ordering::Release);
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        SystemSnapshot {
            version: self.version.to_string(),
            ready: self.ready.load(Ordering::Acquire),
            modules: ["downloads", "inbox", "insight", "music", "stats"]
                .into_iter()
                .map(|id| ModuleDescriptor {
                    id,
                    status: if (id == "downloads" && self.downloads_enabled.load(Ordering::Acquire))
                        || (id == "insight" && self.insight_enabled.load(Ordering::Acquire))
                        || (id == "music" && self.music_enabled.load(Ordering::Acquire))
                        || (id == "stats" && self.stats_enabled.load(Ordering::Acquire))
                        || (id == "inbox" && self.inbox_enabled.load(Ordering::Acquire))
                    {
                        ModuleStatus::Experimental
                    } else {
                        ModuleStatus::Planned
                    },
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_is_shared_but_not_global() {
        let service = SystemService::new("test");
        let other = SystemService::new("test");
        let adapter = service.clone();
        assert!(!adapter.snapshot().ready);
        service.set_ready(true);
        assert!(adapter.snapshot().ready);
        assert!(!other.snapshot().ready);
        service.set_ready(false);
        assert!(!adapter.snapshot().ready);
    }

    #[test]
    fn lifecycle_status_uses_monotonic_age_and_explicit_bot_state() {
        let service = SystemService::new("test");
        let before = service.uptime_seconds();
        service.set_bot_status(true, None);
        assert!(service.uptime_seconds() >= before);
        assert!(service.bot_running());
        assert_eq!(service.bot_username(), None);
        service.set_bot_status(false, None);
        assert!(!service.bot_running());
    }

    #[test]
    fn insight_catalog_follows_shared_configured_runtime() {
        let service = SystemService::new("test");
        let adapter = service.clone();
        service.set_insight_enabled(true);
        let snapshot = adapter.snapshot();
        assert!(!snapshot.ready);
        for module in snapshot.modules {
            let expected = if module.id == "insight" {
                ModuleStatus::Experimental
            } else {
                ModuleStatus::Planned
            };
            assert_eq!(module.status, expected);
        }
        service.set_insight_enabled(false);
        assert!(
            adapter
                .snapshot()
                .modules
                .iter()
                .all(|m| m.status == ModuleStatus::Planned)
        );
        assert!(
            SystemService::new("test")
                .snapshot()
                .modules
                .iter()
                .all(|m| m.status == ModuleStatus::Planned)
        );
    }

    #[test]
    fn catalog_does_not_advertise_unimplemented_features() {
        let snapshot = SystemService::new("test").snapshot();
        assert_eq!(snapshot.modules.len(), 5);
        assert!(
            snapshot
                .modules
                .iter()
                .all(|module| module.status == ModuleStatus::Planned)
        );
    }

    #[test]
    fn product_catalog_tracks_independent_shared_installations() {
        let service = SystemService::new("test");
        let observer = service.clone();
        service.set_music_enabled(true);
        service.set_stats_enabled(true);
        service.set_inbox_enabled(true);
        for module in observer.snapshot().modules {
            assert_eq!(
                module.status,
                if ["music", "stats", "inbox"].contains(&module.id) {
                    ModuleStatus::Experimental
                } else {
                    ModuleStatus::Planned
                }
            );
        }
        service.set_music_enabled(false);
        assert_eq!(
            observer
                .snapshot()
                .modules
                .iter()
                .find(|m| m.id == "music")
                .unwrap()
                .status,
            ModuleStatus::Planned
        );
        assert_eq!(
            observer
                .snapshot()
                .modules
                .iter()
                .find(|m| m.id == "stats")
                .unwrap()
                .status,
            ModuleStatus::Experimental
        );
    }
}
