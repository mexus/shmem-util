use crate::{memmap::MemmapAlloc, shmem_safe::ShmemSafe};
use alloc_collections::{boxes::CustomBox, deque::VecDeque, raw_vec};
use nix::unistd::Pid;
use parking_lot::{Condvar, Mutex, MutexGuard};
use std::{
    alloc::Layout,
    mem::ManuallyDrop,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

/// Communication channel. It is safe to share across process boundaries.
///
/// # Notice
///
/// Please not that the internal queue will be freed when the structure is dropped in the process
/// where the channel has been created.
///
/// In order to destroy the inner queue in a different process, wrap the object into a
/// `ManuallyDrop` helper and call `Channel::destroy(..)` method.
pub struct Channel<T> {
    inner: NonNull<Inner<T>>,
    pid: Pid,
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Self {
        Channel {
            inner: self.inner,
            pid: self.pid,
        }
    }
}

unsafe impl<T: Send> Send for Channel<T> {}
unsafe impl<T: Sync> Sync for Channel<T> {}

impl<T: ShmemSafe> Channel<T> {
    /// Creates an queue with a given capacity in shared memory.
    pub fn new(capacity: usize) -> Result<Self, raw_vec::Error> {
        let queue = VecDeque::<T, _>::with_capacity_in(capacity, MemmapAlloc)?;
        let inner = Inner {
            mutex: Mutex::new(queue),
            receivers: AtomicUsize::new(0),
            senders: AtomicUsize::new(0),
            item_maybe_available: Condvar::new(),
            space_maybe_available: Condvar::new(),
            dropping: AtomicBool::new(false),
        };
        let inner = CustomBox::new_in(inner, MemmapAlloc)
            .map_err(|e| raw_vec::Error::Allocation { source: e })?;
        let (inner, _layout, _allocator) = inner.into_raw_parts();

        Ok(Channel {
            inner: inner.cast(),
            pid: Pid::this(),
        })
    }

    /// Creates a new sender.
    pub fn make_sender(&self) -> Sender<T> {
        let sender = Sender { inner: self.inner };
        unsafe { self.inner.as_ref() }
            .senders
            .fetch_add(1, Ordering::SeqCst);
        sender
    }

    /// Creates a new receiver.
    pub fn make_receiver(&self) -> Receiver<T> {
        let receiver = Receiver { inner: self.inner };
        unsafe { self.inner.as_ref() }
            .receivers
            .fetch_add(1, Ordering::SeqCst);
        receiver
    }

    /// Manually destroys all the internal structures from ANY process id.
    ///
    /// # Safety
    ///
    /// This method is safe to call when only copy of this object is available, because calling
    /// `make_sender` or `make_receiver` after calling `destroy` will cause undefined behaviour.
    pub unsafe fn destroy(mut this: ManuallyDrop<Self>) {
        this.pid = Pid::this();
        let _this = ManuallyDrop::into_inner(this);
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        const TIMEOUT: Duration = Duration::from_secs(30);

        let inner = unsafe { self.inner.as_ref() };
        if self.pid != Pid::this() {
            // This it not an owner of the queue.
            // No-op.
            return;
        }
        inner.dropping.store(true, Ordering::SeqCst);

        let receivers = inner.receivers.swap(0, Ordering::SeqCst);
        let senders = inner.senders.swap(0, Ordering::SeqCst);
        if receivers != 0 {
            log::warn!("There are {} receivers alive", receivers);
            // Let's notify all the receivers that might be blocked on waiting for the mutex.
            inner.space_maybe_available.notify_all();
        }
        if senders != 0 {
            log::warn!("There are {} senders alive", senders);
            // Let's notify all the senders that might be blocked on waiting for the mutex.
            inner.item_maybe_available.notify_all();
        }

        // Let's wait until a probable mutex-holder releases the lock.
        if inner.mutex.try_lock_for(TIMEOUT).is_none() {
            log::error!(
                "Unable to obtain a lock in {:?}. The allocated memory will probably leak.",
                TIMEOUT
            );
        }
        // Here the guard is released, but we are sure we are the only ones who's referencing the
        // `Inner` pointer, because we've set both `senders` and `receivers` atomic to zeroes.
        let _inner = unsafe {
            CustomBox::<Inner<T>, _>::from_raw_parts(
                self.inner.cast(),
                Layout::new::<Inner<T>>(),
                crate::memmap::MemmapAlloc,
            )
        };
    }
}

/// # `fork`-users notice.
///
/// Please not that this structure MUST NOT be used in a process differes from the one where it
/// was created!
///
/// In order to move an object of this type to another process, first call `untie()` and then
/// `tie()` after the fork in the child process.
pub struct Sender<T> {
    inner: NonNull<Inner<T>>,
}

unsafe impl<T: ShmemSafe> ShmemSafe for Sender<T> {}
unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Sync> Sync for Sender<T> {}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let sender = Sender { inner: self.inner };
        unsafe { sender.inner.as_ref() }
            .senders
            .fetch_add(1, Ordering::SeqCst);
        sender
    }
}

/// A `Sender` that is "untied" from the process where it was created.
pub struct UntiedSender<T>(ManuallyDrop<Sender<T>>);

impl<T> UntiedSender<T> {
    /// "Ties" a sender to the current process.
    pub fn tie(self) -> Sender<T> {
        // We "clone" the sender in order to increase atomic counters.
        Sender::clone(&self.0)
    }
}

impl<T> Clone for UntiedSender<T> {
    fn clone(&self) -> Self {
        UntiedSender(ManuallyDrop::clone(&self.0))
    }
}

fn atomic_checked_sub(variable: &AtomicUsize) {
    let mut counter = variable.load(Ordering::Relaxed);
    loop {
        let new_value = match counter.checked_sub(1) {
            Some(x) => x,
            None => {
                // This probably means the original `Channel` structure was destroyed. It's okay.
                eprintln!("Wow, already zero!");
                return;
            }
        };
        match variable.compare_exchange_weak(
            counter,
            new_value,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                // Yay, we did it!
                return;
            }
            Err(new_counter) => {
                // Well, then we need to try again.
                counter = new_counter;
                continue;
            }
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let senders = &unsafe { self.inner.as_ref() }.senders;
        atomic_checked_sub(senders);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TrySendError<T> {
    /// Time out reached while trying to send a message.
    TimedOut(T),

    /// No receivers available.
    NoReceivers(T),
}

impl<T> Sender<T> {
    /// Tries to send data once.
    #[inline(always)]
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        unsafe { self.inner.as_ref() }.try_send_once(item)
    }

    /// Tries to send data until a timeout is reached.
    #[inline(always)]
    pub fn try_send_until(&self, item: T, until: Instant) -> Result<(), TrySendError<T>> {
        unsafe { self.inner.as_ref() }.try_send_until(item, until)
    }

    /// When no receivers are available, returns `Err(item)`.
    #[inline(always)]
    pub fn send(&self, mut item: T) -> Result<(), T> {
        const TIMEOUT: Duration = Duration::from_micros(10);
        let inner = unsafe { self.inner.as_ref() };
        loop {
            // We try to send items with a timeout instead of try-to-lock-forever in order to
            // detect if all the receivers are dropped.
            match inner.try_send_until(item, Instant::now() + TIMEOUT) {
                Ok(()) => return Ok(()),
                Err(TrySendError::TimedOut(repeat)) => {
                    item = repeat;
                    continue;
                }
                Err(TrySendError::NoReceivers(item)) => return Err(item),
            }
        }
    }

    /// Tries to send data until a timeout is reached.
    #[inline(always)]
    pub fn try_send_timeout(&self, item: T, timeout: Duration) -> Result<(), TrySendError<T>> {
        self.try_send_until(item, Instant::now() + timeout)
    }

    /// "Unties" the sender from the current process.
    pub fn untie(self) -> UntiedSender<T> {
        UntiedSender(ManuallyDrop::new(self))
    }
}

/// # `fork`-users notice.
///
/// Please not that this structure MUST NOT be used in a process differes from the one where it
/// was created!
///
/// In order to move an object of this type to another process, first call `untie()` and then
/// `tie()` after the fork in the child process.
pub struct Receiver<T> {
    inner: NonNull<Inner<T>>,
}

unsafe impl<T: ShmemSafe> ShmemSafe for Receiver<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Sync> Sync for Receiver<T> {}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let receiver = Receiver { inner: self.inner };
        unsafe { receiver.inner.as_ref() }
            .receivers
            .fetch_add(1, Ordering::SeqCst);
        receiver
    }
}

/// A `Receiver` that is "untied" from the process where it was created.
pub struct UntiedReceiver<T>(ManuallyDrop<Receiver<T>>);

impl<T> UntiedReceiver<T> {
    /// "Ties" a receiver to the current process.
    ///
    /// Will panic if called from the parent process.
    pub fn tie(self) -> Receiver<T> {
        // We "clone" receiver in order to increase atomic counters.
        Receiver::clone(&self.0)
    }
}

impl<T> Clone for UntiedReceiver<T> {
    fn clone(&self) -> Self {
        UntiedReceiver(self.0.clone())
    }
}

impl<T> Receiver<T> {
    /// Receives an item from the channel.
    ///
    /// If there are no senders left, `Err(())` is returned.
    #[inline(always)]
    pub fn receive(&self) -> Option<T> {
        const TIMEOUT: Duration = Duration::from_micros(10);
        let inner = unsafe { self.inner.as_ref() };
        loop {
            // We try to send items with a timeout instead of try-to-lock-forever in order to
            // detect if all the senders are dropped.
            match inner.try_receive_until(Instant::now() + TIMEOUT) {
                Ok(item) => return Some(item),
                Err(TryReceiveError::TimedOut) => continue,
                Err(TryReceiveError::NoSenders) => return None,
            }
        }
    }

    /// Tries to receive an item once.
    #[inline(always)]
    pub fn try_receive(&self) -> Result<T, TryReceiveError> {
        unsafe { self.inner.as_ref() }.try_receive_once()
    }

    /// Tries to receive an item once within a given timeout.
    #[inline(always)]
    pub fn try_receive_until(&self, until: Instant) -> Result<T, TryReceiveError> {
        unsafe { self.inner.as_ref() }.try_receive_until(until)
    }

    /// Tries to receive an item once within a given timeout.
    #[inline(always)]
    pub fn try_receive_timeout(&self, timeout: Duration) -> Result<T, TryReceiveError> {
        unsafe { self.inner.as_ref() }.try_receive_until(Instant::now() + timeout)
    }

    /// "Unties" the receiver from the current process.
    pub fn untie(self) -> UntiedReceiver<T> {
        UntiedReceiver(ManuallyDrop::new(self))
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let receivers = &unsafe { self.inner.as_ref() }.receivers;
        atomic_checked_sub(receivers);
    }
}

struct Inner<T> {
    mutex: Mutex<VecDeque<T, MemmapAlloc>>,
    item_maybe_available: Condvar,
    space_maybe_available: Condvar,

    receivers: AtomicUsize,
    senders: AtomicUsize,
    dropping: AtomicBool,
}

#[derive(Debug, Clone, Copy)]
pub enum TryReceiveError {
    /// Time out reached while trying to receive a message.
    TimedOut,

    /// No senders available.
    NoSenders,
}

impl<T> Inner<T> {
    #[inline(always)]
    fn push(
        &self,
        guard: &mut MutexGuard<VecDeque<T, MemmapAlloc>>,
        item: T,
    ) -> Result<(), raw_vec::Error> {
        guard.push_back(item)?;
        self.item_maybe_available.notify_one();
        Ok(())
    }

    #[inline(always)]
    fn pop(&self, guard: &mut MutexGuard<VecDeque<T, MemmapAlloc>>) -> Option<T> {
        let item = guard.pop_front()?;
        self.space_maybe_available.notify_one();
        Some(item)
    }

    /// Tries to receive an item until a timeout is reached.
    pub fn try_receive_until(&self, until: Instant) -> Result<T, TryReceiveError> {
        // Let's check that the main process haven't started destruction process.
        if self.dropping.load(Ordering::SeqCst) {
            return Err(TryReceiveError::NoSenders);
        }
        let mut guard = match self.mutex.try_lock_until(until) {
            None => return Err(TryReceiveError::TimedOut),
            Some(guard) => guard,
        };
        loop {
            if let Some(item) = self.pop(&mut guard) {
                return Ok(item);
            } else if self.senders.load(Ordering::SeqCst) == 0
                || self.dropping.load(Ordering::SeqCst)
            {
                return Err(TryReceiveError::NoSenders);
            } else if self
                .item_maybe_available
                .wait_until(&mut guard, until)
                .timed_out()
            {
                return Err(TryReceiveError::TimedOut);
            }
        }
    }

    /// Tries to receive an item once.
    pub fn try_receive_once(&self) -> Result<T, TryReceiveError> {
        // Let's check that the main process haven't started destruction process.
        if self.dropping.load(Ordering::SeqCst) {
            return Err(TryReceiveError::NoSenders);
        }
        let mut guard = match self.mutex.try_lock() {
            None => return Err(TryReceiveError::TimedOut),
            Some(guard) => guard,
        };
        if let Some(item) = self.pop(&mut guard) {
            Ok(item)
        } else if self.senders.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst) {
            Err(TryReceiveError::NoSenders)
        } else {
            Err(TryReceiveError::TimedOut)
        }
    }

    /// Tries to send an item until a timeout is reached.
    pub fn try_send_until(&self, item: T, until: Instant) -> Result<(), TrySendError<T>> {
        // Let's check that the main process haven't started destruction process.
        if self.dropping.load(Ordering::SeqCst) {
            return Err(TrySendError::NoReceivers(item));
        }
        let mut guard = match self.mutex.try_lock_until(until) {
            None => return Err(TrySendError::TimedOut(item)),
            Some(guard) => guard,
        };
        while guard.remaining_capacity() == 0 {
            if self.receivers.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst) {
                return Err(TrySendError::NoReceivers(item));
            } else if self
                .space_maybe_available
                .wait_until(&mut guard, until)
                .timed_out()
            {
                return Err(TrySendError::TimedOut(item));
            }
        }
        self.push(&mut guard, item).expect("Shouldn't fail");
        Ok(())
    }

    /// Tries to send an item once.
    pub fn try_send_once(&self, item: T) -> Result<(), TrySendError<T>> {
        // Let's check that the main process haven't started destruction process.
        if self.dropping.load(Ordering::SeqCst) {
            return Err(TrySendError::NoReceivers(item));
        }
        let mut guard = match self.mutex.try_lock() {
            Some(guard) => guard,
            None => return Err(TrySendError::TimedOut(item)),
        };
        if guard.remaining_capacity() == 0 {
            Err(TrySendError::TimedOut(item))
        } else if self.receivers.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst)
        {
            Err(TrySendError::NoReceivers(item))
        } else {
            self.push(&mut guard, item).expect("Shouldn't fail");
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use nix::unistd::{fork, ForkResult};
    #[test]
    fn check() {
        let channel = Channel::<usize>::new(1234).unwrap();

        let processor = channel.make_receiver();
        let sender = channel.make_sender().untie();

        match fork().unwrap() {
            ForkResult::Parent { child: _ } => {
                let value = processor.receive().unwrap();
                assert_eq!(value, 1234);
            }
            ForkResult::Child => {
                let sender = sender.tie();
                sender.send(1234).unwrap();
            }
        }
    }
}
