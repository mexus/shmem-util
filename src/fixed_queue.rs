//! Fixed queue.

use crate::memmap::MemmapAlloc;
use alloc_collections::{boxes::CustomBox, deque::VecDeque, raw_vec};
use core::ptr::NonNull;
use parking_lot::Mutex;

struct Inner<T>(Mutex<VecDeque<T, MemmapAlloc>>);

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

    /// Tries to push an item to the back of the queue.
    pub fn push_back(&self, item: T) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.push_back(item)
    }

    /// Tries to push an item to the front of the queue.
    pub fn push_front(&self, item: T) -> Result<(), T> {
        unsafe { self.inner.as_ref() }.push_front(item)
    }

    /// Tries to pop an element from the back of queue.
    pub fn pop_back(&self) -> Option<T> {
        unsafe { self.inner.as_ref() }.pop_back()
    }

    /// Tries to pop an element from the front of queue.
    pub fn pop_front(&self) -> Option<T> {
        unsafe { self.inner.as_ref() }.pop_front()
    }
}

impl<T> Inner<T> {
    pub fn new(capacity: usize) -> Result<Self, raw_vec::Error> {
        let queue = VecDeque::<T, _>::with_capacity_in(capacity, MemmapAlloc)?;
        let queue = Mutex::new(queue);
        Ok(Inner(queue))
    }

    pub fn push_back(&self, item: T) -> Result<(), T> {
        let mut queue = self.0.lock();
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            let _ = queue.push_back(item);
            Ok(())
        }
    }

    pub fn push_front(&self, item: T) -> Result<(), T> {
        let mut queue = self.0.lock();
        if queue.remaining_capacity() == 0 {
            Err(item)
        } else {
            let _ = queue.push_front(item);
            Ok(())
        }
    }

    pub fn pop_back(&self) -> Option<T> {
        let mut queue = self.0.lock();
        queue.pop_back()
    }

    pub fn pop_front(&self) -> Option<T> {
        let mut queue = self.0.lock();
        queue.pop_front()
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
                shared_queue.push_back(105324).unwrap();
                thread::sleep(Duration::from_millis(100));

                assert_eq!(Some(100500), shared_queue.pop_back());
            }
            ForkResult::Child => {
                thread::sleep(Duration::from_millis(50));

                assert_eq!(Some(105324), shared_queue.pop_front());
                shared_queue.push_back(100500).unwrap();
            }
        }
    }
}
