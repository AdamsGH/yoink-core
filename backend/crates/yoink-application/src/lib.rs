//! Application services shared by transports. No framework or database types.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
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
}

impl SystemService {
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: Arc::from(version.into()),
            ready: Arc::new(AtomicBool::new(false)),
            downloads_enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }

    /// Advertises only the implemented progressive-video slice, not download parity.
    pub fn set_downloads_enabled(&self, enabled: bool) {
        self.downloads_enabled.store(enabled, Ordering::Release);
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        SystemSnapshot {
            version: self.version.to_string(),
            ready: self.ready.load(Ordering::Acquire),
            modules: ["downloads", "inbox", "insight", "music", "stats"]
                .into_iter()
                .map(|id| ModuleDescriptor {
                    id,
                    status: if id == "downloads" && self.downloads_enabled.load(Ordering::Acquire) {
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
}
