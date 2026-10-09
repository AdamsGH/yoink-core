//! Process-local fixed-key observations shared by the composition root.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Copy)]
pub enum Counter {
    ApiRequests,
    ApiErrors,
    Downloads,
    DownloadsOk,
    DownloadsError,
    DownloadBytes,
    RateLimited,
    CacheHits,
}
impl Counter {
    fn name(self) -> &'static str {
        match self {
            Self::ApiRequests => "api_requests_total",
            Self::ApiErrors => "api_errors_total",
            Self::Downloads => "downloads_total",
            Self::DownloadsOk => "downloads_ok",
            Self::DownloadsError => "downloads_error",
            Self::DownloadBytes => "download_bytes_total",
            Self::RateLimited => "rate_limited",
            Self::CacheHits => "cache_hits",
        }
    }
}
#[derive(Clone, Copy)]
pub enum Histogram {
    ApiResponse,
    DownloadDuration,
}
impl Histogram {
    fn name(self) -> &'static str {
        match self {
            Self::ApiResponse => "api_response_seconds",
            Self::DownloadDuration => "download_duration_seconds",
        }
    }
}
#[derive(Clone, Debug, Default)]
struct Observation {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum: f64,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub uptime_seconds: f64,
    pub counters: BTreeMap<&'static str, u64>,
    pub histograms: BTreeMap<&'static str, HistogramSnapshot>,
}
#[derive(Debug, Default)]
struct State {
    counters: BTreeMap<&'static str, u64>,
    histograms: BTreeMap<&'static str, Observation>,
}
#[derive(Clone, Debug)]
pub struct Metrics {
    started: Arc<Instant>,
    state: Arc<Mutex<State>>,
}
impl Default for Metrics {
    fn default() -> Self {
        Self::new(Arc::new(Instant::now()))
    }
}
fn rounded(value: f64, places: usize) -> f64 {
    // Decimal formatting preserves Python's ties-to-even rounding of the binary sample.
    format!("{value:.places$}").parse().unwrap_or(value)
}
impl Metrics {
    pub(crate) fn new(started: Arc<Instant>) -> Self {
        Self {
            started,
            state: Arc::new(Mutex::new(State::default())),
        }
    }
    pub fn increment(&self, name: Counter, value: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let count = state.counters.entry(name.name()).or_default();
        *count = count.saturating_add(value);
    }
    pub fn observe(&self, name: Histogram, value: f64) {
        if !value.is_finite() || value < 0.0 {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let observation = state.histograms.entry(name.name()).or_default();
        if !(observation.sum + value).is_finite() || observation.count == u64::MAX {
            return;
        }
        if observation.count == 0 {
            observation.min = value;
            observation.max = value;
        }
        observation.count += 1;
        observation.sum += value;
        observation.min = observation.min.min(value);
        observation.max = observation.max.max(value);
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_at(self.started.elapsed().as_secs_f64())
    }
    fn snapshot_at(&self, uptime: f64) -> Snapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            uptime_seconds: rounded(uptime, 1),
            counters: state.counters.clone(),
            histograms: state
                .histograms
                .iter()
                .filter(|(_, v)| v.count > 0)
                .map(|(name, v)| {
                    (
                        *name,
                        HistogramSnapshot {
                            count: v.count,
                            sum: rounded(v.sum, 3),
                            avg: rounded(v.sum / v.count as f64, 3),
                            min: rounded(v.min, 3),
                            max: rounded(v.max, 3),
                        },
                    )
                })
                .collect(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_ties_and_large_samples_match_legacy_rounding() {
        assert_eq!(rounded(1.25, 1), 1.2);
        assert_eq!(rounded(1.75, 1), 1.8);
        assert_eq!(rounded(1.2345, 3), 1.234);
        assert_eq!(rounded(2.675, 2), 2.67);
        assert_eq!(rounded(f64::MAX, 3), f64::MAX);
        let m = Metrics::default();
        m.observe(Histogram::ApiResponse, f64::MAX);
        m.observe(Histogram::ApiResponse, f64::MAX);
        assert_eq!(m.snapshot().histograms["api_response_seconds"].count, 1);
        m.increment(Counter::ApiRequests, u64::MAX);
        m.increment(Counter::ApiRequests, 1);
        assert_eq!(m.snapshot().counters["api_requests_total"], u64::MAX);
    }
    #[test]
    fn snapshots_round_real_observations_and_reject_invalid_samples() {
        let m = Metrics::default();
        assert!(m.snapshot().histograms.is_empty());
        m.observe(Histogram::ApiResponse, 0.12345);
        m.observe(Histogram::ApiResponse, 0.23456);
        m.observe(Histogram::ApiResponse, f64::NAN);
        m.observe(Histogram::ApiResponse, -1.0);
        let s = m.snapshot_at(1.26);
        assert_eq!(s.uptime_seconds, 1.3);
        let h = &s.histograms["api_response_seconds"];
        assert_eq!(
            (h.count, h.sum, h.avg, h.min, h.max),
            (2, 0.358, 0.179, 0.123, 0.235)
        );
        assert!(Metrics::default().snapshot().histograms.is_empty());
    }
    #[test]
    fn clones_share_concurrent_updates_without_dynamic_labels() {
        let m = Metrics::default();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let m = m.clone();
            tasks.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    m.increment(Counter::ApiRequests, 1);
                    m.observe(Histogram::ApiResponse, 0.001);
                }
            }));
        }
        for task in tasks {
            task.join().unwrap();
        }
        let s = m.snapshot();
        assert_eq!(s.counters["api_requests_total"], 8000);
        assert_eq!(s.histograms["api_response_seconds"].count, 8000);
        assert_eq!(s.counters.len(), 1);
        assert_eq!(s.histograms.len(), 1);
    }
}
