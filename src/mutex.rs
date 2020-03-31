use crate::strerror;
use crate::{allocator::ShmemAlloc, memmap::MemmapAlloc, time::UnixClock, ShmemSafe};
use alloc_collections::boxes::CustomBox;
use nix::unistd::Pid;
use std::{
    cell::UnsafeCell,
    mem::{ManuallyDrop, MaybeUninit},
    ops::{Deref, DerefMut},
    time::{Duration, Instant},
};

/// Mutex attributes.
struct MutexAttr(libc::pthread_mutexattr_t);

impl MutexAttr {
    /// Initializes default attributes.
    pub fn new() -> Self {
        let mut obj = MaybeUninit::<libc::pthread_mutexattr_t>::uninit();
        unsafe {
            let errno = libc::pthread_mutexattr_init(obj.as_mut_ptr());
            if errno != 0 {
                panic!("Unable to initialize mutex attributes: {}", strerror(errno));
            }
            Self(obj.assume_init())
        }
    }

    /// Sets `PTHREAD_PROCESS_SHARED` attribute.
    pub fn set_shared(mut self) -> Self {
        unsafe {
            let errno =
                libc::pthread_mutexattr_setpshared(&mut self.0, libc::PTHREAD_PROCESS_SHARED);
            if errno != 0 {
                panic!(
                    "Unable to set PTHREAD_PROCESS_SHARED flag on mutex: {}",
                    strerror(errno)
                );
            }
        }
        self
    }
}

impl Drop for MutexAttr {
    fn drop(&mut self) {
        unsafe {
            let errno = libc::pthread_mutexattr_destroy(&mut self.0);
            if errno != 0 {
                log::error!("Unable to destroy mutex attribute: {}", strerror(errno));
            }
        }
    }
}

/// Mutex with PTHREAD_PROCESS_SHARED attribute.
///
/// # Notice
///
/// * This mutex must be put into the shared memory in order to work across processes!
/// * The data is dropped when the mutex is dropped in the process where it was created.
pub struct Mutex<T> {
    raw_mutex: UnsafeCell<libc::pthread_mutex_t>,
    data: ManuallyDrop<UnsafeCell<T>>,
    creator_pid: Pid,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: ShmemSafe> ShmemSafe for CustomBox<Mutex<T>, MemmapAlloc> {}
unsafe impl<T: ShmemSafe> ShmemSafe for CustomBox<Mutex<T>, ShmemAlloc> {}

/// Mutex RAII guard.
///
/// # `fork`-users notice.
///
/// The guard MUST NOT be shared between applications!
///
/// MAKE SURE that no MutexGuard is alive (i.e. not dropped) prior to call `fork()`!.
pub struct MutexGuard<'a, T> {
    pub(crate) raw_mutex: *mut libc::pthread_mutex_t,
    data: &'a UnsafeCell<T>,
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        // The drop implementation differs from `unlock` in logging error instead of panicking.
        unsafe {
            let errno = libc::pthread_mutex_unlock(self.raw_mutex);
            if errno != 0 {
                log::error!("Unable to unlock mutex: {}", strerror(errno));
            } else {
                log::debug!("Mutex unlocked");
            }
        }
    }
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.data.get() }
    }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.data.get() }
    }
}

impl<T> Mutex<T> {
    /// Initializes a mutex.
    pub fn new(data: T) -> Self {
        let attr = MutexAttr::new().set_shared();
        let mut mutex = MaybeUninit::<libc::pthread_mutex_t>::uninit();
        let mutex = unsafe {
            let errno = libc::pthread_mutex_init(mutex.as_mut_ptr(), &attr.0);
            if errno != 0 {
                panic!("Unable to initialize mutex: {}", strerror(errno));
            }
            mutex.assume_init()
        };

        Self {
            data: ManuallyDrop::new(UnsafeCell::new(data)),
            raw_mutex: UnsafeCell::new(mutex),
            creator_pid: Pid::this(),
        }
    }

    /// Locks the mutex.
    ///
    /// # `fork`-users notice.
    ///
    /// The guard MUST NOT be shared between applications!
    ///
    /// MAKE SURE that no MutexGuard is alive (i.e. not dropped) prior to call `fork()`!.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let ptr = self.raw_mutex.get();
        unsafe {
            let errno = libc::pthread_mutex_lock(ptr);
            if errno != 0 {
                panic!("Unable to lock mutex: {}", strerror(errno));
            } else {
                log::debug!("Mutex locked");
            }
        }
        MutexGuard {
            raw_mutex: ptr,
            data: &self.data,
        }
    }

    /// Tries to lock the mutex without blocking.
    ///
    /// # `fork`-users notice.
    ///
    /// The guard MUST NOT be shared between applications!
    ///
    /// MAKE SURE that no MutexGuard is alive (i.e. not dropped) prior to call `fork()`!.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let ptr = self.raw_mutex.get();
        unsafe {
            let errno = libc::pthread_mutex_trylock(ptr);

            if errno == libc::EBUSY {
                return None;
            } else if errno != 0 {
                panic!("Unable to lock mutex: {}", strerror(errno));
            }
        }
        Some(MutexGuard {
            raw_mutex: ptr,
            data: &self.data,
        })
    }

    /// Tries to obtain a lock within a given timeout.
    ///
    /// # `fork`-users notice.
    ///
    /// The guard MUST NOT be shared between applications!
    ///
    /// MAKE SURE that no MutexGuard is alive (i.e. not dropped) prior to call `fork()`!.
    pub fn try_lock_for(&self, duration: Duration) -> Option<MutexGuard<'_, T>> {
        let until = UnixClock::realtime().add(duration);
        self.try_lock_until_internal(until)
    }

    /// Tries to obtain a lock within a given timeout.
    ///
    /// # `fork`-users notice.
    ///
    /// The guard MUST NOT be shared between applications!
    ///
    /// MAKE SURE that no MutexGuard is alive (i.e. not dropped) prior to call `fork()`!.
    pub fn try_lock_until(&self, until: Instant) -> Option<MutexGuard<'_, T>> {
        let until = UnixClock::from(until);
        self.try_lock_until_internal(UnixClock::from(until))
    }

    /// Tries to obtain a lock within a given timeout.
    fn try_lock_until_internal(&self, until: UnixClock) -> Option<MutexGuard<'_, T>> {
        let ptr = self.raw_mutex.get();
        unsafe {
            let errno = libc::pthread_mutex_timedlock(ptr, until.as_ptr());
            if errno == libc::ETIMEDOUT {
                return None;
            } else if errno != 0 {
                panic!("Unable to lock mutex: {}", strerror(errno));
            }
        }
        Some(MutexGuard {
            raw_mutex: ptr,
            data: &self.data,
        })
    }
}

impl<T> Drop for Mutex<T> {
    fn drop(&mut self) {
        if self.creator_pid != Pid::this() {
            /* No op */
            return;
        }
        let errno = unsafe { libc::pthread_mutex_destroy(self.raw_mutex.get()) };
        if errno != 0 {
            log::error!("Unable to destroy mutex: {}", strerror(errno));
            return;
        } else {
            log::debug!("Mutex destroyed.");
        }
        unsafe {
            ManuallyDrop::drop(&mut self.data);
        }
        log::debug!("Associated data dropped.");
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{init_logging, memmap::MemmapAlloc};
    use alloc_collections::boxes::CustomBox;
    use nix::{
        sys::wait::waitpid,
        unistd::{fork, ForkResult},
    };
    use std::{sync::Arc, thread, time::Duration};

    struct Check<'a>(&'a mut usize);

    impl<'a> Drop for Check<'a> {
        fn drop(&mut self) {
            *self.0 += 1;
            log::debug!("Dropping check ({:?})", *self.0);
        }
    }

    #[test]
    fn check_drop() {
        init_logging();

        let mut counter = CustomBox::new_in(0, MemmapAlloc).unwrap();

        let mutex = CustomBox::new_in(Mutex::new(Check(&mut counter)), MemmapAlloc).unwrap();

        match fork().unwrap() {
            ForkResult::Parent { child } => {
                let guard = mutex.lock();
                assert_eq!(*guard.0, 0);
                drop(guard);
                waitpid(child, None).unwrap();

                let guard = mutex.lock();
                assert_eq!(*guard.0, 0);
                drop(guard);
                drop(mutex);
                assert_eq!(*counter, 1);
            }
            ForkResult::Child => {
                let guard = mutex.lock();
                assert_eq!(*guard.0, 0);
            }
        }
    }

    #[test]
    fn check_spawn() {
        init_logging();

        let mutex = Arc::new(Mutex::new(0usize));
        let guard = mutex.lock();

        let child_thread = {
            let mutex = Arc::clone(&mutex);
            thread::spawn(move || {
                let mut guard = mutex.lock();
                *guard += 1;
            })
        };

        assert_eq!(*guard, 0);
        drop(guard);
        thread::sleep(Duration::from_millis(100));
        let guard = mutex.lock();
        assert_eq!(*guard, 1);

        child_thread.join().unwrap();
    }

    #[test]
    fn check_fork() {
        init_logging();

        let mutex = CustomBox::new_in(Mutex::new(0usize), MemmapAlloc).unwrap();

        match fork().unwrap() {
            ForkResult::Parent { child } => {
                let guard = mutex.lock();
                assert_eq!(*guard, 0);
                drop(guard);
                thread::sleep(Duration::from_millis(10));
                let guard = mutex.lock();
                assert_eq!(*guard, 1);
                waitpid(child, None).unwrap();
            }
            ForkResult::Child => {
                let mut guard = mutex.lock();
                *guard += 1;
            }
        }
    }

    #[test]
    fn check_wait_until() {
        init_logging();

        let mutex = CustomBox::new_in(Mutex::new(0usize), MemmapAlloc).unwrap();

        match fork().unwrap() {
            ForkResult::Parent { child } => {
                let guard = mutex.lock();
                assert_eq!(*guard, 0);

                thread::sleep(Duration::from_millis(200));
                drop(guard);

                waitpid(child, None).unwrap();

                let guard = mutex.lock();
                assert_eq!(*guard, 1);
            }
            ForkResult::Child => {
                thread::sleep(Duration::from_millis(20));
                assert!(mutex.try_lock_for(Duration::from_nanos(1)).is_none());
                let mut guard = mutex
                    .try_lock_for(Duration::from_secs(1))
                    .expect("Unable to obtain a lock");
                *guard += 1;
            }
        }
    }
}
