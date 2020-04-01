//! Process-aware atomic reference counter.

use nix::unistd::Pid;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

pub struct PerProcessReferences {
    counter: AtomicUsize,
    pid: AtomicI32,
}

pub enum DecreaseResult {
    YouAreTheLastOne,
    MoveAlong,
}

impl PerProcessReferences {
    /// Initializes a per-process atomic reference counter.
    pub fn new() -> Self {
        PerProcessReferences {
            counter: AtomicUsize::new(1),
            pid: AtomicI32::new(Pid::this().as_raw()),
        }
    }

    /// Increases the counter atomically,
    pub fn increase(&self) {
        let this_pid = Pid::this().as_raw();
        let count = self.counter.load(Ordering::SeqCst);
        if self.pid.swap(this_pid, Ordering::SeqCst) == this_pid {
            self.counter.fetch_add(1, Ordering::SeqCst);
        } else if count == 0 {
            panic!("Unexpected situation: counter equals to zero!");
        } else {
            log::info!("Detected process change; resetting counter to 1");
            self.counter.fetch_sub(count - 1, Ordering::SeqCst);
        }
    }

    /// Returns `true` if it was the last reference.
    pub fn decrease(&self, current_pid: Pid) -> DecreaseResult {
        let current_pid = current_pid.as_raw();
        if self.pid.swap(current_pid, Ordering::SeqCst) == current_pid {
            if self.counter.fetch_sub(1, Ordering::SeqCst) == 1 {
                DecreaseResult::YouAreTheLastOne
            } else {
                DecreaseResult::MoveAlong
            }
        } else {
            // We've tried to decrease a freshly copied-across-fork reference counter.
            DecreaseResult::YouAreTheLastOne
        }
    }
}
