use crate::strerror;
use core::{cmp::Ordering, convert::TryFrom, mem::MaybeUninit, time::Duration};
use std::time::Instant;

/// Monotonic time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnixClock(libc::timespec);

impl UnixClock {
    /// Returns current timestamp, using CLOCK_MONOTONIC.
    pub fn monitonic() -> Self {
        Self::with_clock_id(libc::CLOCK_MONOTONIC)
    }
    /// Returns current timestamp, using CLOCK_REALTIME.
    pub fn realtime() -> Self {
        Self::with_clock_id(libc::CLOCK_REALTIME)
    }

    /// Returns current timestamp using given clock id.
    pub fn with_clock_id(clock_id: libc::clockid_t) -> Self {
        let mut timespec = MaybeUninit::<libc::timespec>::uninit();
        let res = unsafe { libc::clock_gettime(clock_id, timespec.as_mut_ptr()) };
        if res == -1 {
            let errno = nix::errno::errno();
            panic!("Unable to get current time: {}", strerror(errno));
        }
        unsafe { Self(timespec.assume_init()) }
    }

    /// Returns a raw pointer to the underlying timespec structure.
    pub fn as_ptr(&self) -> *const libc::timespec {
        &self.0
    }

    /// Adds duration to the instant.
    pub fn add(self, duration: Duration) -> Self {
        let mut seconds = self.0.tv_sec;
        let mut nanos = self.0.tv_nsec;
        debug_assert!(
            seconds >= 0 && nanos >= 0,
            "sec = {}, nanos = {}",
            seconds,
            nanos
        );

        let duration = duration.as_nanos();

        let add_secs = duration / 1_000_000_000;
        let add_nanos = duration - add_secs * 1_000_000_000;

        let add_secs = i64::try_from(add_secs).expect("Too large duration");
        let add_nanos = add_nanos as i64;

        if nanos + add_nanos >= 1_000_000_000 {
            seconds += add_secs + 1;
            nanos += add_nanos;
            nanos -= 1_000_000_000;
        } else {
            seconds += add_secs;
            nanos += add_nanos;
        }

        Self(libc::timespec {
            tv_nsec: nanos,
            tv_sec: seconds,
        })
    }

    /// Substracts duration from the instant.
    pub fn sub(self, duration: Duration) -> Self {
        let mut seconds = self.0.tv_sec;
        let mut nanos = self.0.tv_nsec;
        debug_assert!(
            seconds >= 0 && nanos >= 0,
            "sec = {}, nanos = {}",
            seconds,
            nanos
        );

        let duration = duration.as_nanos();

        let sub_secs = duration / 1_000_000_000;
        let sub_nanos = duration - sub_secs * 1_000_000_000;

        let sub_secs = i64::try_from(sub_secs).expect("Too large duration");
        let mut sub_nanos = sub_nanos as i64;

        if sub_nanos >= nanos {
            seconds -= 1;
            sub_nanos = 1_000_000_000 - sub_nanos;
        }

        seconds -= sub_secs;
        nanos -= sub_nanos;

        Self(libc::timespec {
            tv_nsec: nanos,
            tv_sec: seconds,
        })
    }
}

impl From<Instant> for UnixClock {
    fn from(value: Instant) -> Self {
        let std_now = Instant::now();
        let monotonic_now = UnixClock::monitonic();

        if let Some(duration) = value.checked_duration_since(std_now) {
            monotonic_now.add(duration)
        } else {
            let duration = std_now.duration_since(value);
            monotonic_now.sub(duration)
        }
    }
}

impl PartialOrd for UnixClock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UnixClock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .tv_sec
            .cmp(&other.0.tv_sec)
            .then_with(|| self.0.tv_nsec.cmp(&other.0.tv_nsec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check() {
        const TIMEOUT: Duration = Duration::from_millis(300);

        let until_monotonic = UnixClock::monitonic().add(TIMEOUT);

        let begin = Instant::now();
        while UnixClock::monitonic() <= until_monotonic {}
        let elapsed = begin.elapsed();

        let diff = elapsed
            .checked_sub(TIMEOUT)
            .unwrap_or_else(|| TIMEOUT - elapsed);
        assert!(diff < Duration::from_millis(10), "{:?}", diff);
    }
}
