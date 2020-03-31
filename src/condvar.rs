use crate::{
    allocator::ShmemAlloc, memmap::MemmapAlloc, mutex::MutexGuard, strerror, time::UnixClock,
    ShmemSafe,
};
use alloc_collections::boxes::CustomBox;
use nix::unistd::Pid;
use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    time::{Duration, Instant},
};

/// Conditional variable.
///
/// # Notice
///
/// This var must be put into the shared memory in order to work across processes!
#[derive(Debug)]
pub struct Condvar {
    raw_condvar: UnsafeCell<libc::pthread_cond_t>,
    creator_pid: Pid,
}

unsafe impl Send for Condvar {}
unsafe impl Sync for Condvar {}
unsafe impl ShmemSafe for CustomBox<Condvar, MemmapAlloc> {}
unsafe impl ShmemSafe for CustomBox<Condvar, ShmemAlloc> {}

#[derive(Debug)]
pub(crate) struct CondAttr(libc::pthread_condattr_t);

impl CondAttr {
    /// Initializes default attributes.
    pub fn new() -> Self {
        let mut obj = MaybeUninit::<libc::pthread_condattr_t>::uninit();
        unsafe {
            let errno = libc::pthread_condattr_init(obj.as_mut_ptr());
            if errno != 0 {
                panic!(
                    "Unable to initialize condvar attributes: {}",
                    strerror(errno)
                );
            }
            CondAttr(obj.assume_init())
        }
    }

    /// Sets `PTHREAD_PROCESS_SHARED` attribute.
    pub fn set_shared(mut self) -> Self {
        let errno =
            unsafe { libc::pthread_condattr_setpshared(&mut self.0, libc::PTHREAD_PROCESS_SHARED) };
        if errno != 0 {
            panic!("Unable to set PTHREAD_PROCESS_SHARED: {}", strerror(errno));
        }
        self
    }

    /// Sets clock attribute to `CLOCK_MONOTONIC`.
    pub fn set_monotonic_clock(mut self) -> Self {
        let errno = unsafe { libc::pthread_condattr_setclock(&mut self.0, libc::CLOCK_MONOTONIC) };
        if errno != 0 {
            panic!("Unable to set CLOCK_MONOTONIC: {}", strerror(errno));
        }
        self
    }
}

impl Drop for CondAttr {
    fn drop(&mut self) {
        let errno = unsafe { libc::pthread_condattr_destroy(&mut self.0) };
        if errno != 0 {
            log::error!(
                "Unable to destroy conditional variable attribute: {}",
                strerror(errno)
            );
        }
    }
}

impl Condvar {
    /// Creates a new conditional variable.
    pub fn new() -> Self {
        let attr = CondAttr::new().set_shared().set_monotonic_clock();
        let mut condvar = MaybeUninit::<libc::pthread_cond_t>::uninit();
        let condvar = unsafe {
            if libc::pthread_cond_init(condvar.as_mut_ptr(), &attr.0) != 0 {
                panic!("Unable to initialize conditional variable");
            }
            condvar.assume_init()
        };

        Self {
            raw_condvar: UnsafeCell::new(condvar),
            creator_pid: Pid::this(),
        }
    }

    /// Notifies the conditional variable in at least one thread/application.
    pub fn signal(&self) {
        let errno = unsafe { libc::pthread_cond_signal(self.raw_condvar.get()) };

        if errno != 0 {
            panic!("pthread_cond_signal failed")
        }
    }

    /// Notifies the conditional variable in all threads/applications.
    pub fn broadcast(&self) {
        let errno = unsafe { libc::pthread_cond_broadcast(self.raw_condvar.get()) };

        if errno != 0 {
            panic!("pthread_cond_broadcast failed: {}", strerror(errno))
        }
    }

    /// Waits for the conditional variable to be notified.
    ///
    /// # Notice
    ///
    /// Spurious wake-ups are possible.
    pub fn wait<'a, T, F>(&self, guard: &mut MutexGuard<'a, T>, mut is_ready: F)
    where
        F: FnMut(&T) -> bool,
    {
        while !is_ready(guard) {
            unsafe {
                let errno = libc::pthread_cond_wait(self.raw_condvar.get(), guard.raw_mutex);
                if errno != 0 {
                    panic!(
                        "Waiting on conditional variable failed: {}",
                        strerror(errno)
                    );
                } else {
                    log::debug!("Mutex guard re-locked");
                }
            }
        }
    }

    /// Waits for the conditional variable to be notified.
    ///
    /// # Notice
    ///
    /// Spurious wake-ups are possible.
    pub fn wait_for<'a, T, F>(
        &self,
        guard: &mut MutexGuard<'a, T>,
        timeout: Duration,
        is_ready: F,
    ) -> Result<(), ()>
    where
        F: FnMut(&T) -> bool,
    {
        let now = UnixClock::monitonic();
        let until = now.add(timeout);

        self.wait_until_internal(guard, until, is_ready)
    }

    /// Waits for the conditional variable to be notified.
    ///
    /// # Notice
    ///
    /// Spurious wake-ups are possible.
    pub fn wait_until<'a, T, F>(
        &self,
        guard: &mut MutexGuard<'a, T>,
        until: Instant,
        is_ready: F,
    ) -> Result<(), ()>
    where
        F: FnMut(&T) -> bool,
    {
        let until = UnixClock::from(until);
        self.wait_until_internal(guard, until, is_ready)
    }

    /// Waits for the conditional variable to be notified.
    ///
    /// # Notice
    ///
    /// Spurious wake-ups are possible.
    fn wait_until_internal<'a, T, F>(
        &self,
        guard: &mut MutexGuard<'a, T>,
        until: UnixClock,
        mut is_ready: F,
    ) -> Result<(), ()>
    where
        F: FnMut(&T) -> bool,
    {
        while !is_ready(&guard) {
            unsafe {
                let errno = libc::pthread_cond_timedwait(
                    self.raw_condvar.get(),
                    guard.raw_mutex,
                    until.as_ptr(),
                );
                if errno == libc::ETIMEDOUT {
                    if is_ready(&guard) {
                        break;
                    } else {
                        return Err(());
                    }
                } else if errno != 0 {
                    panic!(
                        "Waiting on conditional variable failed: {}",
                        strerror(errno)
                    );
                }
            }
        }
        Ok(())
    }
}

impl Drop for Condvar {
    fn drop(&mut self) {
        if self.creator_pid != Pid::this() {
            /* No op */
            return;
        }
        let errno = unsafe { libc::pthread_cond_destroy(self.raw_condvar.get()) };
        if errno != 0 {
            log::error!(
                "Unable to destroy conditional variable: {}",
                strerror(errno)
            );
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{init_logging, memmap::MemmapAlloc, mutex::Mutex};
    use alloc_collections::boxes::CustomBox;
    use crossbeam_utils::thread::scope;
    use nix::{
        sys::wait::waitpid,
        unistd::{fork, ForkResult},
    };
    use std::{sync::Arc, thread, time::Duration};

    #[test]
    fn check_spawn() {
        init_logging();

        // let pair = Arc::new((Mutex::new(0usize), Condvar::new()));
        let mutex = Mutex::new(0usize);
        let condvar = Condvar::new();

        scope(|s| {
            // Child thread.
            s.spawn(|_| {
                let mut guard = mutex.lock();
                log::debug!("Waiting for signal");
                condvar.wait(&mut guard, |value| *value == 1);
                *guard += 1;
                log::debug!("Exiting child thread");
            });

            // Main thread.

            thread::sleep(Duration::from_millis(200));
            {
                let mut guard = mutex.lock();
                assert_eq!(*guard, 0);

                *guard += 1;
                condvar.signal();
            }

            thread::sleep(Duration::from_millis(200));
            {
                let guard = mutex.lock();
                assert_eq!(*guard, 2);
            }
        })
        .unwrap();
        drop(condvar);
        log::debug!("Dropping mutex");
        drop(mutex);
    }

    #[test]
    fn check_fork() {
        init_logging();

        let var = Condvar::new();
        let var = CustomBox::new_in(var, MemmapAlloc).unwrap();

        let data = CustomBox::new_in(0usize, MemmapAlloc).unwrap();
        let mutex = Mutex::new(data);

        match fork().unwrap() {
            ForkResult::Parent { child } => {
                thread::sleep(Duration::from_millis(100));

                let mut guard = mutex.lock();
                assert_eq!(**guard, 0);
                **guard += 1;
                drop(guard);
                var.broadcast();
                log::debug!("[parent] signal sent!");

                waitpid(child, None).unwrap();
                assert_eq!(**mutex.lock(), 2);
            }
            ForkResult::Child => {
                let mut guard = mutex.lock();
                log::debug!("[child] Lock acquired");
                var.wait(&mut guard, |value| **value == 1);
                log::debug!("[child] Notification received");
                **guard += 1;
            }
        }
    }

    #[test]
    fn check_wait_timeout() {
        let pair = Arc::new((Condvar::new(), Mutex::new(0usize)));

        let child_thread = {
            let pair = Arc::clone(&pair);
            thread::spawn(move || {
                let (var, mutex) = &*pair;
                let mut guard = mutex.lock();
                let res = var.wait_for(&mut guard, Duration::from_millis(100), |&value| value == 1);
                assert!(res.is_err());
                let res = var.wait_until(
                    &mut guard,
                    Instant::now() + Duration::from_millis(100),
                    |&value| value == 1,
                );
                assert!(res.is_err());
            })
        };
        child_thread.join().unwrap();
    }
}
