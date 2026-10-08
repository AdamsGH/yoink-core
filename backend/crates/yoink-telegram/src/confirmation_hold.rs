//! Fixture-only exact durable-token receipt barrier; never holds a DB connection.
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldState {
    Armed,
    Entered,
    Released,
    Expired,
    Dropped,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldStatus {
    pub token: String,
    pub state: HoldState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmError {
    Invalid,
    Outstanding,
}
struct Slot {
    status: HoldStatus,
    deadline: Instant,
}
struct Inner {
    slot: Mutex<Option<Slot>>,
    replay: Mutex<Option<(String, u64)>>,
    changed: watch::Sender<u64>,
}
#[derive(Clone)]
pub struct ConfirmationHold(Arc<Inner>);
impl Default for ConfirmationHold {
    fn default() -> Self {
        Self(Arc::new(Inner {
            slot: Mutex::new(None),
            replay: Mutex::new(None),
            changed: watch::channel(0).0,
        }))
    }
}
fn expire(slot: &mut Slot) {
    if matches!(slot.status.state, HoldState::Armed | HoldState::Entered)
        && Instant::now() >= slot.deadline
    {
        slot.status.state = HoldState::Expired;
    }
}
impl ConfirmationHold {
    /// Observe one exact replay token. Repeated reads never reset its counter.
    pub fn replay_status(&self, token: &str) -> Result<u64, ArmError> {
        if token.is_empty() || token.len() > 512 {
            return Err(ArmError::Invalid);
        }
        let mut replay = self.0.replay.lock().expect("replay observation");
        match replay.as_ref() {
            Some((observed, count)) if observed == token => Ok(*count),
            Some(_) => Err(ArmError::Outstanding),
            None => {
                *replay = Some((token.into(), 0));
                Ok(0)
            }
        }
    }

    pub(crate) fn record_replay(&self, token: &str) {
        let mut replay = self.0.replay.lock().expect("replay observation");
        if let Some((observed, count)) = replay.as_mut()
            && observed == token
        {
            *count = count.saturating_add(1);
        }
    }

    /// One outstanding exact token, at most 512 bytes, held for (0,120s].
    /// Deadline starts at arm, not entry. Terminal status remains until rearmed.
    pub fn arm(&self, token: &str, duration: Duration) -> Result<(), ArmError> {
        if token.is_empty()
            || token.len() > 512
            || duration.is_zero()
            || duration > Duration::from_secs(120)
        {
            return Err(ArmError::Invalid);
        }
        let mut slot = self.0.slot.lock().expect("confirmation hold");
        if let Some(s) = slot.as_mut() {
            expire(s);
            if matches!(s.status.state, HoldState::Armed | HoldState::Entered) {
                return Err(ArmError::Outstanding);
            }
        }
        *slot = Some(Slot {
            status: HoldStatus {
                token: token.into(),
                state: HoldState::Armed,
            },
            deadline: Instant::now() + duration,
        });
        self.0.changed.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
    }
    /// Synchronous: no async/DB lock or network work.
    pub fn status(&self) -> Option<HoldStatus> {
        let mut slot = self.0.slot.lock().expect("confirmation hold");
        slot.as_mut().map(|s| {
            expire(s);
            s.status.clone()
        })
    }
    pub fn release(&self, token: &str) -> bool {
        let mut slot = self.0.slot.lock().expect("confirmation hold");
        let Some(s) = slot.as_mut() else {
            return false;
        };
        expire(s);
        if s.status.token != token
            || !matches!(s.status.state, HoldState::Armed | HoldState::Entered)
        {
            return false;
        }
        s.status.state = HoldState::Released;
        self.0.changed.send_modify(|v| *v = v.wrapping_add(1));
        true
    }
    pub(crate) async fn enter(&self, token: &str) {
        let mut changed = self.0.changed.subscribe();
        let deadline = {
            let mut slot = self.0.slot.lock().expect("confirmation hold");
            let Some(s) = slot.as_mut() else {
                return;
            };
            expire(s);
            if s.status.token != token || s.status.state != HoldState::Armed {
                return;
            }
            s.status.state = HoldState::Entered;
            s.deadline
        };
        let _guard = Entered(self.clone(), token.into(), deadline);
        loop {
            {
                let mut slot = self.0.slot.lock().expect("confirmation hold");
                let Some(s) = slot.as_mut() else {
                    return;
                };
                expire(s);
                if s.status.token != token
                    || s.deadline != deadline
                    || s.status.state != HoldState::Entered
                {
                    return;
                }
            }
            tokio::select! { _ = tokio::time::sleep_until(deadline) => { let _ = self.status(); return; }, _ = changed.changed() => {} }
        }
    }
}
struct Entered(ConfirmationHold, String, Instant);
impl Drop for Entered {
    fn drop(&mut self) {
        let mut slot = self.0.0.slot.lock().expect("confirmation hold");
        if let Some(s) = slot.as_mut() {
            expire(s);
            if s.status.token == self.1
                && s.deadline == self.2
                && s.status.state == HoldState::Entered
            {
                s.status.state = HoldState::Dropped;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_observation_is_exact_bounded_shared_and_read_stable() {
        let hold = ConfirmationHold::default();
        hold.record_replay("x");
        assert_eq!(hold.replay_status(""), Err(ArmError::Invalid));
        assert_eq!(hold.replay_status(&"x".repeat(513)), Err(ArmError::Invalid));
        assert_eq!(hold.replay_status("x"), Ok(0));
        hold.record_replay("other");
        assert_eq!(hold.replay_status("x"), Ok(0));
        hold.clone().record_replay("x");
        assert_eq!(hold.replay_status("x"), Ok(1));
        assert_eq!(hold.replay_status("x"), Ok(1));
        assert_eq!(hold.replay_status("other"), Err(ArmError::Outstanding));
        assert_eq!(ConfirmationHold::default().replay_status("x"), Ok(0));
    }

    #[tokio::test(start_paused = true)]
    async fn exact_bounded_expiry_release_and_drop() {
        let hold = ConfirmationHold::default();
        assert_eq!(
            hold.arm("x", Duration::from_secs(121)),
            Err(ArmError::Invalid)
        );
        hold.arm("x", Duration::from_secs(120)).unwrap();
        hold.enter("other").await;
        assert_eq!(hold.status().unwrap().state, HoldState::Armed);
        let clone = hold.clone();
        let task = tokio::spawn(async move { clone.enter("x").await });
        tokio::task::yield_now().await;
        assert_eq!(hold.status().unwrap().state, HoldState::Entered);
        task.abort();
        let _ = task.await;
        assert_eq!(hold.status().unwrap().state, HoldState::Dropped);
        hold.arm("x", Duration::from_secs(1)).unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(hold.status().unwrap().state, HoldState::Expired);
        hold.arm("x", Duration::from_secs(1)).unwrap();
        assert!(!hold.release("other"));
        assert!(hold.release("x"));
        assert_eq!(hold.status().unwrap().state, HoldState::Released);
        hold.arm("entered", Duration::from_secs(120)).unwrap();
        assert_eq!(
            hold.arm("second", Duration::from_secs(1)),
            Err(ArmError::Outstanding)
        );
        let wait = hold.enter("entered");
        tokio::pin!(wait);
        assert!(futures::poll!(&mut wait).is_pending());
        assert!(hold.clone().release("entered"));
        wait.await;
        assert_eq!(hold.status().unwrap().state, HoldState::Released);
    }
}
