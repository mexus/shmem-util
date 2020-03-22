//! Fixed queue.

use crate::memmap::MemmapAlloc;
use alloc_collections::{boxes::CustomBox, deque::VecDeque, raw_vec};
use core::ptr::NonNull;
use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};

struct Inner<T> {
    mutex: Mutex<VecDeque<T, MemmapAlloc>>,
    pushed: Condvar,
    popped: Condvar,
}

pub struct FixedQueue<T> {
    inner: NonNull<Inner<T>>,
}

unsafe impl<T: Send> Send for FixedQueue<T> {}
unsafe impl<T: Sync> Sync for FixedQueue<T> {}

impl<T> Clone for FixedQueue<T> {
    fn clone(&self) -> Self {
        FixedQueue { inner: self.inner }
    }
}

impl<T> FixedQueue<T> {
    /// Creates a fixed-sized queue in shared memory.
    pub fn new(capacity: usize) -> Result<Self, raw_vec::Error> {
        let inner = Inner::<T>::new(capacity)?;
        let boxed = CustomBox::new_in(inner, MemmapAlloc)
            .map_err(|source| raw_vec::Error::Allocation { source })?;
        let (inner, ..) = boxed.into_raw_parts();

        Ok(FixedQueue {
            inner: inner.cast(),
        })
    }
}

impl<T> FixedQueue<T> {
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

impl<T> FixedQueue<T> {
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

impl<T> FixedQueue<T> {
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

impl<T> FixedQueue<T> {
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
        let pushed = Condvar::new();
        let popped = Condvar::new();
        Ok(Inner {
            mutex: queue,
            pushed,
            popped,
        })
    }
}

impl<T> Inner<T> {
    pub fn push_back(&self, item: T) {
        let mut queue = self.mutex.lock();
        loop {
            if queue.remaining_capacity() != 0 {
                break;
            }
            self.popped.wait(&mut queue);
        }
        let _ = queue.push_back(item);
        self.pushed.notify_one();
    }

    pub fn try_push_back(&self, item: T) -> Result<(), T> {
        let mut queue = self.mutex.lock();
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            let _ = queue.push_back(item);
            self.pushed.notify_one();
            Ok(())
        }
    }

    pub fn try_push_back_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        let until = Instant::now() + timeout;
        self.try_push_back_until(item, until)
    }

    pub fn try_push_back_until(&self, item: T, until: Instant) -> Result<(), T> {
        let mut queue = self.mutex.lock();
        while Instant::now() < until {
            if queue.remaining_capacity() != 0 {
                break;
            }
            self.popped.wait_until(&mut queue, until);
        }
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            let _ = queue.push_back(item);
            self.pushed.notify_one();
            Ok(())
        }
    }
}

impl<T> Inner<T> {
    pub fn push_front(&self, item: T) {
        let mut queue = self.mutex.lock();
        loop {
            if queue.remaining_capacity() != 0 {
                break;
            }
            self.popped.wait(&mut queue);
        }
        let _ = queue.push_front(item);
        self.pushed.notify_one();
    }

    pub fn try_push_front(&self, item: T) -> Result<(), T> {
        let mut queue = self.mutex.lock();
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            let _ = queue.push_front(item);
            self.pushed.notify_one();
            Ok(())
        }
    }

    pub fn try_push_front_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        let until = Instant::now() + timeout;
        self.try_push_front_until(item, until)
    }

    pub fn try_push_front_until(&self, item: T, until: Instant) -> Result<(), T> {
        let mut queue = self.mutex.lock();
        while Instant::now() < until {
            if queue.remaining_capacity() != 0 {
                break;
            }
            self.popped.wait_until(&mut queue, until);
        }
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            let _ = queue.push_front(item);
            self.pushed.notify_one();
            Ok(())
        }
    }
}

impl<T> Inner<T> {
    pub fn pop_back(&self) -> T {
        let mut queue = self.mutex.lock();
        if let Some(item) = queue.pop_back() {
            self.popped.notify_one();
            item
        } else {
            self.pushed.wait(&mut queue);
            let item = queue.pop_back().expect("Error while popping");
            self.popped.notify_one();
            item
        }
    }

    pub fn try_pop_back(&self) -> Option<T> {
        let mut queue = self.mutex.lock();
        let item = queue.pop_back()?;
        self.popped.notify_one();
        Some(item)
    }

    pub fn try_pop_back_timeout(&self, timeout: Duration) -> Option<T> {
        let until = Instant::now() + timeout;
        self.try_pop_back_until(until)
    }

    pub fn try_pop_back_until(&self, until: Instant) -> Option<T> {
        let mut queue = self.mutex.lock();
        while Instant::now() < until {
            if queue.capacity() != 0 {
                break;
            }
            self.pushed.wait_until(&mut queue, until);
        }
        let item = queue.pop_back()?;
        self.popped.notify_one();
        Some(item)
    }
}

impl<T> Inner<T> {
    pub fn pop_front(&self) -> T {
        let mut queue = self.mutex.lock();
        if let Some(item) = queue.pop_front() {
            self.popped.notify_one();
            item
        } else {
            self.pushed.wait(&mut queue);
            let item = queue.pop_front().expect("Error while popping");
            self.popped.notify_one();
            item
        }
    }

    pub fn try_pop_front(&self) -> Option<T> {
        let mut queue = self.mutex.lock();
        let item = queue.pop_front()?;
        self.popped.notify_one();
        Some(item)
    }

    pub fn try_pop_front_timeout(&self, timeout: Duration) -> Option<T> {
        let until = Instant::now() + timeout;
        self.try_pop_front_until(until)
    }

    pub fn try_pop_front_until(&self, until: Instant) -> Option<T> {
        let mut queue = self.mutex.lock();
        while Instant::now() < until {
            if queue.capacity() != 0 {
                break;
            }
            self.pushed.wait_until(&mut queue, until);
        }
        let item = queue.pop_front()?;
        self.popped.notify_one();
        Some(item)
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
