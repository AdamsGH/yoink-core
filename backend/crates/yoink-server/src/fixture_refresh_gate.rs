use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use yoink_downloads::profile::ProfileError;

const ARMED: u8 = 0;
const ENTERED: u8 = 1;
const RELEASED: u8 = 2;
const EXPIRED: u8 = 3;
const DROPPED: u8 = 4;
const RESET: u8 = 5;

struct Slot {
    state: AtomicU8,
    claimed: AtomicBool,
    deadline: Instant,
    changed: CancellationToken,
}

#[derive(Default)]
pub struct RefreshGate {
    slot: Mutex<Option<Arc<Slot>>>,
    stopped: CancellationToken,
}

impl RefreshGate {
    pub fn command(&self, mode: &str) -> Result<Value, &'static str> {
        let mut current = self.slot.lock().expect("fixture refresh gate");
        if let Some(slot) = current
            .as_ref()
            .filter(|slot| Instant::now() >= slot.deadline)
        {
            for state in [ARMED, ENTERED] {
                slot.state
                    .compare_exchange(state, EXPIRED, Ordering::SeqCst, Ordering::SeqCst)
                    .ok();
            }
        }
        match mode {
            "oauth_refresh_arm" => self.arm(&mut current, Duration::from_secs(90))?,
            "oauth_refresh_state" => {}
            "oauth_refresh_release" => {
                let slot = current.as_ref().ok_or("refresh_gate_not_armed")?;
                if Instant::now() >= slot.deadline {
                    slot.state
                        .compare_exchange(ARMED, EXPIRED, Ordering::SeqCst, Ordering::SeqCst)
                        .ok();
                    slot.state
                        .compare_exchange(ENTERED, EXPIRED, Ordering::SeqCst, Ordering::SeqCst)
                        .ok();
                }
                if slot
                    .state
                    .compare_exchange(ENTERED, RELEASED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return Err("refresh_gate_not_entered");
                }
                slot.changed.cancel();
            }
            "oauth_refresh_reset" => {
                if let Some(slot) = current.take() {
                    slot.state.store(RESET, Ordering::SeqCst);
                    slot.changed.cancel();
                }
            }
            _ => return Err("invalid_refresh_gate_command"),
        }
        Ok(Self::snapshot(current.as_ref()))
    }

    fn arm(&self, current: &mut Option<Arc<Slot>>, duration: Duration) -> Result<(), &'static str> {
        if self.stopped.is_cancelled() {
            return Err("refresh_gate_stopped");
        }
        if current
            .as_ref()
            .is_some_and(|slot| matches!(slot.state.load(Ordering::SeqCst), ARMED | ENTERED))
        {
            return Err("refresh_gate_outstanding");
        }
        *current = Some(Arc::new(Slot {
            state: AtomicU8::new(ARMED),
            claimed: AtomicBool::new(false),
            deadline: Instant::now() + duration,
            changed: CancellationToken::new(),
        }));
        Ok(())
    }

    fn snapshot(slot: Option<&Arc<Slot>>) -> Value {
        let state = slot.map(|slot| slot.state.load(Ordering::SeqCst));
        json!({"ok":true,"armed":state == Some(ARMED),"entered":state == Some(ENTERED),
            "phase":match state { Some(ARMED)=>"armed",Some(ENTERED)=>"entered",Some(RELEASED)=>"released",Some(EXPIRED)=>"expired",Some(DROPPED)=>"dropped",_=>"idle" }})
    }

    pub fn stop(&self) {
        self.stopped.cancel();
    }

    pub async fn wait(&self) -> Result<(), ProfileError> {
        if self.stopped.is_cancelled() {
            return Err(ProfileError::Unavailable);
        }
        let slot = self.slot.lock().expect("fixture refresh gate").clone();
        let Some(slot) = slot else {
            return Ok(());
        };
        if slot.claimed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        if self.stopped.is_cancelled() || Instant::now() >= slot.deadline {
            slot.state.store(EXPIRED, Ordering::SeqCst);
            return Err(ProfileError::Unavailable);
        }
        if slot
            .state
            .compare_exchange(ARMED, ENTERED, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(ProfileError::Unavailable);
        }
        let _entered = Entered(slot.clone());
        tokio::select! {
            biased;
            _ = self.stopped.cancelled() => Err(ProfileError::Unavailable),
            _ = tokio::time::sleep_until(slot.deadline) => {
                slot.state.compare_exchange(ENTERED, EXPIRED, Ordering::SeqCst, Ordering::SeqCst).ok();
                Err(ProfileError::Unavailable)
            },
            _ = slot.changed.cancelled() => {
                if slot.state.load(Ordering::SeqCst) == RELEASED { Ok(()) }
                else { Err(ProfileError::Unavailable) }
            },
        }
    }
}

struct Entered(Arc<Slot>);
impl Drop for Entered {
    fn drop(&mut self) {
        self.0
            .state
            .compare_exchange(ENTERED, DROPPED, Ordering::SeqCst, Ordering::SeqCst)
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_barrier_release_reset_drop_expiry_and_shutdown() {
        let gate = RefreshGate::default();
        gate.command("oauth_refresh_arm").unwrap();
        assert!(gate.command("oauth_refresh_arm").is_err());
        let mut held = Box::pin(gate.wait());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(held.as_mut().poll(cx).is_pending()))
                .await
        );
        assert_eq!(
            gate.command("oauth_refresh_state").unwrap()["entered"],
            true
        );
        gate.command("oauth_refresh_release").unwrap();
        held.await.unwrap();
        gate.command("oauth_refresh_arm").unwrap();
        let mut held = Box::pin(gate.wait());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(held.as_mut().poll(cx).is_pending()))
                .await
        );
        drop(held);
        assert_eq!(
            gate.command("oauth_refresh_state").unwrap()["phase"],
            "dropped"
        );
        gate.command("oauth_refresh_arm").unwrap();
        let mut held = Box::pin(gate.wait());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(held.as_mut().poll(cx).is_pending()))
                .await
        );
        gate.command("oauth_refresh_reset").unwrap();
        assert!(held.await.is_err());
        gate.arm(&mut gate.slot.lock().unwrap(), Duration::from_millis(1))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(
            gate.command("oauth_refresh_state").unwrap()["phase"],
            "expired"
        );
        gate.command("oauth_refresh_arm").unwrap();
        gate.command("oauth_refresh_reset").unwrap();
        gate.arm(&mut gate.slot.lock().unwrap(), Duration::from_millis(1))
            .unwrap();
        assert!(gate.wait().await.is_err());
        assert_eq!(
            gate.command("oauth_refresh_state").unwrap()["phase"],
            "expired"
        );
        gate.command("oauth_refresh_arm").unwrap();
        let mut held = Box::pin(gate.wait());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(held.as_mut().poll(cx).is_pending()))
                .await
        );
        gate.stop();
        assert!(held.await.is_err());
        assert!(gate.command("oauth_refresh_arm").is_err());
    }
}
