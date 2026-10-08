//! Shared, permanently closing handler admission. Tokens cover all awaited work.
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct Handlers(Arc<Inner>);
struct Inner {
    state: Mutex<State>,
    changed: watch::Sender<usize>,
}
struct State {
    closing: bool,
    count: usize,
}
pub(crate) struct Handler(Handlers);
impl Handlers {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                closing: false,
                count: 0,
            }),
            changed: watch::channel(0).0,
        }))
    }
    pub(crate) fn admit(&self) -> Option<Handler> {
        let mut state = self.0.state.lock().expect("handler registry");
        if state.closing {
            return None;
        }
        state.count += 1;
        Some(Handler(self.clone()))
    }
    pub(crate) fn close(&self) {
        self.0.state.lock().expect("handler registry").closing = true;
    }
    pub(crate) async fn drain(&self) {
        let mut changed = self.0.changed.subscribe();
        self.close();
        loop {
            if self.0.state.lock().expect("handler registry").count == 0 {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }
}
impl Drop for Handler {
    fn drop(&mut self) {
        let mut state = self.0.0.state.lock().expect("handler registry");
        state.count -= 1;
        self.0.0.changed.send_replace(state.count);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shared_close_cancelled_waiters_and_abort() {
        let handlers = Handlers::new();
        let token = handlers.admit().unwrap();
        let clone = handlers.clone();
        let task = tokio::spawn(async move {
            let _token = token;
            std::future::pending::<()>().await;
        });
        let waiter = tokio::spawn(async move { clone.drain().await });
        tokio::task::yield_now().await;
        assert!(handlers.admit().is_none());
        assert!(!waiter.is_finished());
        waiter.abort();
        task.abort();
        let _ = task.await;
        tokio::join!(handlers.drain(), handlers.drain());
        assert!(handlers.admit().is_none());
    }
    #[tokio::test]
    async fn moving_token_never_opens_empty_gap() {
        let handlers = Handlers::new();
        let token = handlers.admit().unwrap();
        let token = Some(token); // ownership transfer, not deregister/register
        handlers.close();
        let wait = handlers.drain();
        tokio::pin!(wait);
        assert!(futures::poll!(&mut wait).is_pending());
        drop(token);
        wait.await;
    }
}
