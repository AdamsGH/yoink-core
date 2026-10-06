//! Bounded transient lookup lifecycle; no admissions or persisted actor state.
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const MAX_TRANSIENT_QUERIES: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub struct InlineSearchConfig {
    pub concurrency: usize,
    pub timeout: Duration,
    pub requests_per_second: u32,
    pub actor_interval: Duration,
}
impl Default for InlineSearchConfig {
    fn default() -> Self {
        Self {
            concurrency: 3,
            timeout: Duration::from_secs(20),
            requests_per_second: 10,
            actor_interval: Duration::from_millis(300),
        }
    }
}
impl InlineSearchConfig {
    pub fn valid(self) -> bool {
        (1..=8).contains(&self.concurrency)
            && (1..=120).contains(&self.timeout.as_secs())
            && self.timeout.subsec_nanos() == 0
            && (1..=100).contains(&self.requests_per_second)
            && (Duration::from_millis(1)..=Duration::from_secs(10)).contains(&self.actor_interval)
    }
}
/// One budget from admission; cancelling IO waits must not drop a live executor.
#[derive(Clone)]
pub(super) struct QueryLifetime {
    pub cancel: CancellationToken,
    pub deadline: Instant,
}
impl QueryLifetime {
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
    pub async fn wait<T>(&self, future: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => None,
            _ = tokio::time::sleep_until(self.deadline) => { self.cancel.cancel(); None },
            value = future => Some(value),
        }
    }
}

struct Actor {
    last: Instant,
    cancel: CancellationToken,
}
struct State {
    actors: HashMap<u64, Actor>,
    window: Instant,
    count: u32,
}
#[derive(Clone)]
pub(super) struct Searches {
    pub config: InlineSearchConfig,
    pub slots: Arc<Semaphore>,
    pub tasks: TaskTracker,
    pub stop: CancellationToken,
    state: Arc<Mutex<State>>,
    task_slots: Arc<Semaphore>,
}
impl Searches {
    pub fn new(config: InlineSearchConfig, stop: CancellationToken) -> Self {
        Self {
            config,
            slots: Arc::new(Semaphore::new(config.concurrency)),
            tasks: TaskTracker::new(),
            stop,
            task_slots: Arc::new(Semaphore::new(MAX_TRANSIENT_QUERIES)),
            state: Arc::new(Mutex::new(State {
                actors: HashMap::new(),
                window: Instant::now(),
                count: 0,
            })),
        }
    }
    /// Supersede even throttled queries. A cancelled older query can never answer.
    pub fn begin(&self, actor: u64) -> Option<(QueryLifetime, bool)> {
        let now = Instant::now();
        let mut state = self.state.lock().ok()?;
        if let Some(previous) = state.actors.get(&actor) {
            previous.cancel.cancel();
        }
        if self.stop.is_cancelled() {
            return None;
        }
        if now.duration_since(state.window) >= Duration::from_secs(1) {
            state.window = now;
            state.count = 0;
        }
        let allowed = state.count < self.config.requests_per_second
            && !state
                .actors
                .get(&actor)
                .is_some_and(|old| now.duration_since(old.last) < self.config.actor_interval);
        let last = if allowed {
            now
        } else {
            state.actors.get(&actor).map_or(now, |old| old.last)
        };
        // Fixed memory ceiling; only finished/cancelled lookups can be evicted.
        if state.actors.len() >= MAX_TRANSIENT_QUERIES && !state.actors.contains_key(&actor) {
            let evict = state
                .actors
                .iter()
                .filter(|(_, entry)| entry.cancel.is_cancelled())
                .min_by_key(|(_, entry)| entry.last)
                .map(|(actor, _)| *actor)?;
            state.actors.remove(&evict);
        }
        let cancel = self.stop.child_token();
        state.actors.insert(
            actor,
            Actor {
                last,
                cancel: cancel.clone(),
            },
        );
        if allowed {
            state.count += 1;
        }
        Some((
            QueryLifetime {
                cancel,
                deadline: now + self.config.timeout,
            },
            allowed,
        ))
    }
    pub fn spawn(&self, lifetime: QueryLifetime, run: impl Future<Output = ()> + Send + 'static) {
        let Ok(permit) = self.task_slots.clone().try_acquire_owned() else {
            lifetime.cancel.cancel();
            return;
        };
        self.tasks.spawn(async move {
            let _permit = permit;
            tokio::pin!(run);
            tokio::select! {
                biased;
                _ = lifetime.cancel.cancelled() => { run.await; },
                _ = tokio::time::sleep_until(lifetime.deadline) => {
                    lifetime.cancel.cancel();
                    // Do not abort/drop the process future: cancellation must reap
                    // the actual worker and remove its workspace before completion.
                    run.await;
                },
                () = &mut run => {},
            }
            lifetime.cancel.cancel();
        });
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn tracked_response_capacity_includes_throttled_queries_and_pending_cleanup() {
        let searches = Searches::new(InlineSearchConfig::default(), CancellationToken::new());
        let (cleanup, cleanup_wait) = tokio::sync::watch::channel(false);
        for actor in 1..=MAX_TRANSIENT_QUERIES as u64 {
            let (lifetime, _) = searches.begin(actor).unwrap();
            let cancel = lifetime.cancel.clone();
            let mut cleanup_wait = cleanup_wait.clone();
            searches.spawn(lifetime, async move {
                cancel.cancelled().await;
                cleanup_wait.changed().await.unwrap();
            });
        }
        assert_eq!(searches.tasks.len(), MAX_TRANSIENT_QUERIES);
        let (overflow, allowed) = searches.begin(1).unwrap();
        assert!(!allowed);
        searches.spawn(overflow.clone(), async {
            panic!("overflow task was polled")
        });
        assert!(overflow.cancel.is_cancelled());
        assert_eq!(searches.tasks.len(), MAX_TRANSIENT_QUERIES);
        searches.stop.cancel();
        tokio::task::yield_now().await;
        assert_eq!(searches.tasks.len(), MAX_TRANSIENT_QUERIES);
        cleanup.send(true).unwrap();
        searches.shutdown().await;
        assert!(searches.tasks.is_empty());
        assert_eq!(
            searches.task_slots.available_permits(),
            MAX_TRANSIENT_QUERIES
        );
    }

    #[tokio::test(start_paused = true)]
    async fn admission_deadline_is_cumulative_through_held_answer() {
        let searches = Searches::new(
            InlineSearchConfig {
                timeout: Duration::from_secs(10),
                ..InlineSearchConfig::default()
            },
            CancellationToken::new(),
        );
        let admitted = Instant::now();
        let (life, _) = searches.begin(42).unwrap();
        let (stage, mut stages) = tokio::sync::mpsc::unbounded_channel();
        let (auth, auth_wait) = tokio::sync::oneshot::channel::<()>();
        let (lang, lang_wait) = tokio::sync::oneshot::channel::<()>();
        let (extract, extract_wait) = tokio::sync::oneshot::channel::<()>();
        let task_life = life.clone();
        searches.spawn(life.clone(), async move {
            for (name, gate) in [
                ("auth", auth_wait),
                ("language", lang_wait),
                ("extraction", extract_wait),
            ] {
                stage.send((name, task_life.remaining())).unwrap();
                assert!(task_life.wait(gate).await.unwrap().is_ok());
            }
            stage.send(("answer", task_life.remaining())).unwrap();
            assert!(task_life.wait(std::future::pending::<()>()).await.is_none());
        });
        assert_eq!(
            stages.recv().await.unwrap(),
            ("auth", Duration::from_secs(10))
        );
        tokio::time::advance(Duration::from_secs(3)).await;
        auth.send(()).unwrap();
        assert_eq!(
            stages.recv().await.unwrap(),
            ("language", Duration::from_secs(7))
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        lang.send(()).unwrap();
        assert_eq!(
            stages.recv().await.unwrap(),
            ("extraction", Duration::from_secs(5))
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        extract.send(()).unwrap();
        assert_eq!(
            stages.recv().await.unwrap(),
            ("answer", Duration::from_secs(3))
        );
        tokio::time::advance(Duration::from_secs(3)).await;
        searches.tasks.close();
        searches.tasks.wait().await;
        assert_eq!(Instant::now() - admitted, Duration::from_secs(10));
        assert!(life.cancel.is_cancelled());
        assert!(searches.tasks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_cancels_but_waits_for_worker_cleanup_instead_of_aborting_future() {
        let searches = Searches::new(InlineSearchConfig::default(), CancellationToken::new());
        let (life, _) = searches.begin(42).unwrap();
        let (cancel_seen, seen) = tokio::sync::oneshot::channel();
        let (cleanup, cleanup_wait) = tokio::sync::oneshot::channel();
        let child = life.cancel.clone();
        searches.spawn(life.clone(), async move {
            child.cancelled().await;
            cancel_seen.send(()).unwrap();
            cleanup_wait.await.unwrap();
        });
        tokio::time::advance(searches.config.timeout).await;
        seen.await.unwrap();
        assert!(life.cancel.is_cancelled());
        assert_eq!(
            searches.tasks.len(),
            1,
            "cleanup must remain scoped to the tracked task"
        );
        cleanup.send(()).unwrap();
        searches.tasks.close();
        searches.tasks.wait().await;
        assert!(searches.tasks.is_empty());
    }

    #[tokio::test]
    async fn supersession_rate_capacity_and_shutdown_are_bounded() {
        let config = InlineSearchConfig {
            concurrency: 1,
            actor_interval: Duration::from_millis(1),
            requests_per_second: 2,
            ..InlineSearchConfig::default()
        };
        let searches = Searches::new(config, CancellationToken::new());
        let (first, allowed) = searches.begin(42).unwrap();
        assert!(allowed);
        tokio::time::sleep(config.actor_interval).await;
        let (second, allowed) = searches.begin(42).unwrap();
        assert!(allowed);
        assert!(first.cancel.is_cancelled());
        assert!(!searches.begin(42).unwrap().1);
        assert!(second.cancel.is_cancelled());
        assert!(!searches.begin(43).unwrap().1);
        let permit = searches.slots.clone().try_acquire_owned().unwrap();
        assert!(searches.slots.clone().try_acquire_owned().is_err());
        drop(permit);
        let token = searches.stop.child_token();
        searches.tasks.spawn(async move {
            token.cancelled().await;
        });
        searches.shutdown().await;
        assert!(searches.tasks.is_empty());
        assert!(searches.begin(44).is_none());
        assert!(
            !InlineSearchConfig {
                concurrency: 0,
                ..config
            }
            .valid()
        );
        assert!(
            !InlineSearchConfig {
                timeout: Duration::ZERO,
                ..config
            }
            .valid()
        );
    }
}
