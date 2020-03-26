//! Fixed queue.

use crate::{allocator::ShmemAlloc, memmap::MemmapAlloc};
use alloc_collections::{boxes::CustomBox, deque::VecDeque, raw_vec, Alloc, IndexMap, Vec};
use core::{alloc::Layout, ptr::NonNull};
use nix::unistd::Pid;
use parking_lot::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Marker trait for types that are safe to be transmitted between processes.
pub unsafe trait ShmemSafe {}

unsafe impl ShmemSafe for u8 {}
unsafe impl ShmemSafe for u16 {}
unsafe impl ShmemSafe for u32 {}
unsafe impl ShmemSafe for u64 {}
unsafe impl ShmemSafe for u128 {}
unsafe impl ShmemSafe for usize {}

unsafe impl ShmemSafe for i8 {}
unsafe impl ShmemSafe for i16 {}
unsafe impl ShmemSafe for i32 {}
unsafe impl ShmemSafe for i64 {}
unsafe impl ShmemSafe for i128 {}
unsafe impl ShmemSafe for isize {}

unsafe impl ShmemSafe for char {}

unsafe impl<T: ShmemSafe> ShmemSafe for VecDeque<T, MemmapAlloc> {}
unsafe impl<T: ShmemSafe> ShmemSafe for Vec<T, MemmapAlloc> {}
unsafe impl<T: ShmemSafe> ShmemSafe for IndexMap<T, MemmapAlloc> {}

unsafe impl<T: ShmemSafe> ShmemSafe for VecDeque<T, ShmemAlloc> {}
unsafe impl<T: ShmemSafe> ShmemSafe for Vec<T, ShmemAlloc> {}
unsafe impl<T: ShmemSafe> ShmemSafe for IndexMap<T, ShmemAlloc> {}

struct Inner<T> {
    mutex: Mutex<VecDeque<T, MemmapAlloc>>,
    item_available: Condvar,
    space_available: Condvar,
}

pub struct FixedQueue<T: ShmemSafe> {
    inner: NonNull<Inner<T>>,
    creator_pid: Pid,
    owner: bool,
}

unsafe impl<T: Send + ShmemSafe> Send for FixedQueue<T> {}
unsafe impl<T: Sync + ShmemSafe> Sync for FixedQueue<T> {}
unsafe impl<T: ShmemSafe> ShmemSafe for FixedQueue<T> {}

impl<T: ShmemSafe> Clone for FixedQueue<T> {
    fn clone(&self) -> Self {
        FixedQueue {
            inner: self.inner,
            creator_pid: self.creator_pid,
            owner: false,
        }
    }
}

impl<T: ShmemSafe> FixedQueue<T> {
    /// Creates a fixed-sized queue in shared memory.
    pub fn new(capacity: usize) -> Result<Self, raw_vec::Error> {
        let inner = Inner::<T>::new(capacity)?;
        let boxed = CustomBox::new_in(inner, MemmapAlloc)
            .map_err(|source| raw_vec::Error::Allocation { source })?;
        let (inner, ..) = boxed.into_raw_parts();

        Ok(FixedQueue {
            inner: inner.cast(),
            creator_pid: Pid::this(),
            owner: true,
        })
    }

    /// Prevents this queue from being uninitialized when it's dropped in the process that created
    /// the queue.
    ///
    /// If called on the original object, returns a `FixedQueueDestructor` that might be called
    /// from any process to free the memory occupied by the queue.
    pub fn keep(&mut self) -> Option<FixedQueueDestructor<T>> {
        if self.owner && self.creator_pid == Pid::this() {
            self.creator_pid = Pid::from_raw(0);
            Some(FixedQueueDestructor(self.inner))
        } else {
            None
        }
    }

    /// Attemps to lock the internal mutex within a given timeout, and forcibly unlocks it if the
    /// attempt fails.
    ///
    /// This method is intended to be used when a child process holding the mutex dies before
    /// releasing the lock, so it is advised to call this method if and only if the parent process
    /// (i.e. the one that created the queue) receives `SIGCHLD` signal, notifying its child was
    /// killed/dumped/etc.
    ///
    /// # Safety
    ///
    /// Calling this method can cause undefined behaviour when there is a live process holding the
    /// lock.
    pub unsafe fn check_recover(&mut self, timeout: Duration) {
        self.inner.as_mut().check_recover(timeout)
    }
}

/// A queue destroyer.
#[must_use = "The queue won't be freed unless `destroy` is called!"]
pub struct FixedQueueDestructor<T>(NonNull<Inner<T>>);

impl<T> FixedQueueDestructor<T> {
    /// # Safety
    ///
    /// The caller must ensure that no other instances of the queue exist
    /// (in other processes too!).
    pub unsafe fn destroy(self) {
        let _ = CustomBox::<Inner<T>, _>::from_raw_parts(
            self.0.cast(),
            Layout::new::<Inner<T>>(),
            MemmapAlloc,
        );
    }
}

unsafe impl<T: Send> Send for FixedQueueDestructor<T> {}
unsafe impl<T: Sync> Sync for FixedQueueDestructor<T> {}

impl<T: ShmemSafe> Drop for FixedQueue<T> {
    fn drop(&mut self) {
        if let Some(destructor) = self.keep() {
            unsafe { destructor.destroy() }
        }
    }
}

impl<T: ShmemSafe> FixedQueue<T> {
    /// Pushes an item to the back of the queue.
    pub fn push_back(&self, item: T) {
        unsafe { self.inner.as_ref() }.push_back(item)
    }

    /// Tries to push an item to the back of the queue.
    pub fn try_push_back(&self, item: T) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.try_push_back(item)
    }

    /// Tries to push an item to the back of the queue within a given timeout.
    pub fn try_push_back_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.try_push_back_timeout(item, timeout)
    }

    /// Tries to push an item to the back of the queue within a given timeout.
    pub fn try_push_back_until(&self, item: T, until: Instant) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.try_push_back_until(item, until)
    }
}

impl<T: ShmemSafe> FixedQueue<T> {
    /// Pushes an item to the front of the queue.
    pub fn push_front(&self, item: T) {
        unsafe { self.inner.as_ref() }.push_front(item)
    }

    /// Tries to push an item to the front of the queue.
    pub fn try_push_front(&self, item: T) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.try_push_front(item)
    }

    /// Tries to push an item to the front of the queue within a given timeout.
    pub fn try_push_front_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.try_push_front_timeout(item, timeout)
    }

    /// Tries to push an item to the front of the queue within a given timeout.
    pub fn try_push_front_until(&self, item: T, until: Instant) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.try_push_front_until(item, until)
    }
}

impl<T: ShmemSafe> FixedQueue<T> {
    /// Pops an element from the back of queue.
    pub fn pop_back(&self) -> T {
        unsafe { self.inner.as_ref() }.pop_back()
    }

    /// Tries to pop an element from the back of queue.
    pub fn try_pop_back(&self) -> Option<T> {
        unsafe { self.inner.as_ref() }.try_pop_back()
    }

    /// Tries to pop an element from the back of queue within a given timeout.
    pub fn try_pop_back_timeout(&self, timeout: Duration) -> Option<T> {
        unsafe { self.inner.as_ref() }.try_pop_back_timeout(timeout)
    }

    /// Tries to pop an element from the back of queue within a given timeout.
    pub fn try_pop_back_until(&self, until: Instant) -> Option<T> {
        unsafe { self.inner.as_ref() }.try_pop_back_until(until)
    }
}

impl<T: ShmemSafe> FixedQueue<T> {
    /// Pops an element from the front of queue.
    pub fn pop_front(&self) -> T {
        unsafe { self.inner.as_ref() }.pop_front()
    }

    /// Tries to pop an element from the front of queue.
    pub fn try_pop_front(&self) -> Option<T> {
        unsafe { self.inner.as_ref() }.try_pop_front()
    }

    /// Tries to pop an element from the front of queue within a given timeout.
    pub fn try_pop_front_timeout(&self, timeout: Duration) -> Option<T> {
        unsafe { self.inner.as_ref() }.try_pop_front_timeout(timeout)
    }

    /// Tries to pop an element from the front of queue within a given timeout.
    pub fn try_pop_front_until(&self, until: Instant) -> Option<T> {
        unsafe { self.inner.as_ref() }.try_pop_front_until(until)
    }
}

impl<T> Inner<T> {
    pub fn new(capacity: usize) -> Result<Self, raw_vec::Error> {
        let queue = VecDeque::<T, _>::with_capacity_in(capacity, MemmapAlloc)?;
        let queue = Mutex::new(queue);
        Ok(Inner {
            mutex: queue,
            item_available: Condvar::new(),
            space_available: Condvar::new(),
        })
    }

    /// Tries to lock the internal mutex for a given amount of time and forcibly unlocks it
    /// if the timeout expires.
    /// 
    /// # Safety
    /// 
    /// The method is only safe to call when there are no active processes accessing the
    /// mutex-protecred data.
    pub unsafe fn check_recover(&mut self, timeout: Duration) {
        if self.mutex.try_lock_for(timeout).is_none() {
            lock_api::RawMutex::unlock(self.mutex.raw());
        }
    }

    fn push_back_inner<A: Alloc>(&self, mut guard: MutexGuard<'_, VecDeque<T, A>>, item: T) {
        let r = guard.push_back(item);
        debug_assert!(r.is_ok());
        if guard.len() == 1 {
            // This means we've pushed the first element
            self.item_available.notify_one();
        }
    }

    fn push_front_inner<A: Alloc>(&self, mut guard: MutexGuard<'_, VecDeque<T, A>>, item: T) {
        let r = guard.push_front(item);
        debug_assert!(r.is_ok());
        if guard.len() == 1 {
            // This means we've pushed the first element
            self.item_available.notify_one();
        }
    }

    fn pop_back_inner<A: Alloc>(&self, mut queue: MutexGuard<'_, VecDeque<T, A>>) -> Option<T> {
        if let Some(item) = queue.pop_back() {
            if queue.remaining_capacity() == 1 {
                // This means we've just made a space for others to push.
                self.space_available.notify_one();
            }
            Some(item)
        } else {
            None
        }
    }

    fn pop_front_inner<A: Alloc>(&self, mut queue: MutexGuard<'_, VecDeque<T, A>>) -> Option<T> {
        if let Some(item) = queue.pop_front() {
            if queue.remaining_capacity() == 1 {
                // This means we've just made a space for others to push.
                self.space_available.notify_one();
            }
            Some(item)
        } else {
            None
        }
    }
}

impl<T> Inner<T> {
    pub fn push_back(&self, item: T) {
        let mut queue = self.mutex.lock();
        if queue.remaining_capacity() == 0 {
            self.space_available.wait(&mut queue);
        }
        self.push_back_inner(queue, item)
    }

    pub fn try_push_back(&self, item: T) -> Result<(), T> {
        let queue = match self.mutex.try_lock() {
            Some(guard) => guard,
            None => return Err(item),
        };
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            self.push_back_inner(queue, item);
            Ok(())
        }
    }

    pub fn try_push_back_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        let until = Instant::now() + timeout;
        self.try_push_back_until(item, until)
    }

    pub fn try_push_back_until(&self, item: T, until: Instant) -> Result<(), T> {
        let mut queue = match self.mutex.try_lock_until(until) {
            Some(guard) => guard,
            None => return Err(item),
        };
        if queue.remaining_capacity() == 0 {
            if self
                .space_available
                .wait_until(&mut queue, until)
                .timed_out()
            {
                return Err(item);
            }
        }
        self.push_back_inner(queue, item);
        Ok(())
    }
}

impl<T> Inner<T> {
    pub fn push_front(&self, item: T) {
        let mut queue = self.mutex.lock();
        if queue.remaining_capacity() == 0 {
            self.space_available.wait(&mut queue);
        }
        self.push_front_inner(queue, item)
    }

    pub fn try_push_front(&self, item: T) -> Result<(), T> {
        let queue = match self.mutex.try_lock() {
            Some(guard) => guard,
            None => return Err(item),
        };
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            self.push_front_inner(queue, item);
            Ok(())
        }
    }

    pub fn try_push_front_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        let until = Instant::now() + timeout;
        self.try_push_front_until(item, until)
    }

    pub fn try_push_front_until(&self, item: T, until: Instant) -> Result<(), T> {
        let mut queue = match self.mutex.try_lock_until(until) {
            Some(guard) => guard,
            None => return Err(item),
        };
        if queue.remaining_capacity() == 0 {
            if self
                .space_available
                .wait_until(&mut queue, until)
                .timed_out()
            {
                return Err(item);
            }
        }
        self.push_front_inner(queue, item);
        Ok(())
    }
}

impl<T> Inner<T> {
    pub fn pop_back(&self) -> T {
        let mut queue = self.mutex.lock();
        if queue.len() == 0 {
            self.item_available.wait(&mut queue);
        }

        self.pop_back_inner(queue).expect("Error while popping")
    }

    pub fn try_pop_back(&self) -> Option<T> {
        let queue = self.mutex.try_lock()?;
        self.pop_back_inner(queue)
    }

    pub fn try_pop_back_timeout(&self, timeout: Duration) -> Option<T> {
        let until = Instant::now() + timeout;
        self.try_pop_back_until(until)
    }

    pub fn try_pop_back_until(&self, until: Instant) -> Option<T> {
        let mut queue = self.mutex.try_lock_until(until)?;
        if queue.len() == 0 {
            if self
                .item_available
                .wait_until(&mut queue, until)
                .timed_out()
            {
                return None;
            }
        }
        Some(self.pop_back_inner(queue).expect("Error while popping"))
    }
}

impl<T> Inner<T> {
    pub fn pop_front(&self) -> T {
        let mut queue = self.mutex.lock();
        if queue.len() == 0 {
            self.item_available.wait(&mut queue);
        }
        self.pop_front_inner(queue).expect("Pop failed")
    }

    pub fn try_pop_front(&self) -> Option<T> {
        let queue = self.mutex.try_lock()?;
        self.pop_front_inner(queue)
    }

    pub fn try_pop_front_timeout(&self, timeout: Duration) -> Option<T> {
        let until = Instant::now() + timeout;
        self.try_pop_front_until(until)
    }

    pub fn try_pop_front_until(&self, until: Instant) -> Option<T> {
        let mut queue = self.mutex.try_lock_until(until)?;
        if queue.len() == 0 {
            if self
                .item_available
                .wait_until(&mut queue, until)
                .timed_out()
            {
                return None;
            }
        }
        Some(self.pop_front_inner(queue).expect("Pop failed"))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use nix::unistd::{fork, ForkResult};
    use std::{thread, time::Duration};

    #[test]
    fn check() {
        let shared_queue = FixedQueue::<usize>::new(1024).unwrap();

        match fork().unwrap() {
            ForkResult::Parent { .. } => {
                shared_queue.push_front(105324);
                thread::sleep(Duration::from_millis(100));
                assert_eq!(100500, shared_queue.pop_back());
            }
            ForkResult::Child => {
                assert_eq!(105324, shared_queue.pop_front());
                shared_queue.push_back(100500);
            }
        }
    }
}
