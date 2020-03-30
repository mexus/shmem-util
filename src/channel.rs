use crate::{
    condvar::Condvar,
    memmap::MemmapAlloc,
    mutex::{Mutex, MutexGuard},
    shmem_safe::ShmemSafe,
};
use alloc_collections::{boxes::CustomBox, deque::VecDeque, raw_vec};
use nix::unistd::Pid;
use snafu::Snafu;
use std::{
    mem::ManuallyDrop,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

struct PerProcessReferences {
    counter: AtomicUsize,
    pid: AtomicI32,
}

enum DecreaseResult {
    YouAreTheLastOne,
    MoveAlong,
}

impl PerProcessReferences {
    fn new() -> Self {
        PerProcessReferences {
            counter: AtomicUsize::new(1),
            pid: AtomicI32::new(Pid::this().as_raw()),
        }
    }

    fn increase(&self) {
        let this_pid = Pid::this().as_raw();
        let count = self.counter.load(Ordering::SeqCst);
        if self.pid.swap(this_pid, Ordering::SeqCst) == this_pid {
            self.counter.fetch_add(1, Ordering::SeqCst);
        } else if count == 0 {
            panic!("Unexpected situation: counter equals to zero!");
        } else {
            log::info!("Detected process change");
            self.counter.fetch_sub(count - 1, Ordering::SeqCst);
        }
    }

    /// Returns `true` if it was the last reference.
    fn decrease(&self, current_pid: Pid) -> DecreaseResult {
        let current_pid = current_pid.as_raw();
        if self.pid.swap(current_pid, Ordering::SeqCst) == current_pid {
            if self.counter.fetch_sub(1, Ordering::SeqCst) == 1 {
                DecreaseResult::YouAreTheLastOne
            } else {
                DecreaseResult::MoveAlong
            }
        } else {
            // We've tried to decrease a freshly copied-across-fork `Channel`.
            DecreaseResult::YouAreTheLastOne
        }
    }
}

/// Communication channel. It is safe to share across process boundaries.
///
/// # Notice
///
/// Please not that the internal queue will be freed when the structure is dropped in the process
/// where the channel has been created.
///
/// In order to destroy the inner queue in a different process, wrap the object into `ManuallyDrop`
/// and call `Channel::destroy(..)` method.
pub struct Channel<T> {
    inner: NonNull<Inner<T>>,

    /// Heap-allocated references.
    references: NonNull<PerProcessReferences>,

    original_pid: Pid,
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Self {
        let references = self.references;
        unsafe { references.as_ref() }.increase();
        Channel {
            inner: self.inner,
            references,
            original_pid: self.original_pid,
        }
    }
}

unsafe impl<T: Send> Send for Channel<T> {}
unsafe impl<T: Sync> Sync for Channel<T> {}

impl<T: ShmemSafe> Channel<T> {
    /// Creates an queue with a given capacity in shared memory.
    pub fn new(capacity: usize) -> Result<Self, raw_vec::Error> {
        let queue = VecDeque::<T, _>::with_capacity_in(capacity, MemmapAlloc)?;
        log::debug!("Queue allocated");
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
        log::debug!("\"Inner\" moved to shared memory");
        let (inner, _allocator) = inner.into_raw_parts();

        // Heap-allocated references.
        let references = Box::new(PerProcessReferences::new());
        let references = unsafe { NonNull::new_unchecked(Box::into_raw(references)) };

        Ok(Channel {
            inner,
            references,
            original_pid: Pid::this(),
        })
    }

    /// Creates a new sender.
    pub fn make_sender(&self) -> Sender<T> {
        let sender = Sender {
            channel: self.clone(),
        };
        unsafe { self.inner.as_ref() }
            .senders
            .fetch_add(1, Ordering::SeqCst);
        sender
    }

    /// Creates a new receiver.
    pub fn make_receiver(&self) -> Receiver<T> {
        let receiver = Receiver {
            channel: self.clone(),
        };
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
        this.original_pid = Pid::this();
        let _this = ManuallyDrop::into_inner(this);
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        const TIMEOUT: Duration = Duration::from_secs(30);

        let inner = unsafe { self.inner.as_ref() };
        let current_pid = Pid::this();

        if let DecreaseResult::MoveAlong = unsafe { self.references.as_ref() }.decrease(current_pid)
        {
            // Not the last reference.
            return;
        }

        if self.original_pid != current_pid {
            // This it not an owner of the queue.
            // No-op.
            return;
        }

        log::debug!("Dropping channel");
        inner.dropping.store(true, Ordering::SeqCst);

        let receivers = inner.receivers.swap(0, Ordering::SeqCst);
        let senders = inner.senders.swap(0, Ordering::SeqCst);
        log::debug!("{} receivers remain, {} senders remain", receivers, senders);
        if receivers != 0 {
            log::warn!("There are {} receivers alive", receivers);
            // Let's notify all the receivers that might be blocked on waiting for data to become
            // available.
            inner.item_maybe_available.broadcast();
        }
        if senders != 0 {
            log::warn!("There are {} senders alive", senders);
            // Let's notify all the senders that might be blocked on waiting for space to become
            // available.
            inner.space_maybe_available.broadcast();
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
        let inner = unsafe {
            CustomBox::<Inner<T>, _>::from_raw_parts(self.inner, crate::memmap::MemmapAlloc)
        };
        drop(inner);
        log::debug!("Dropped the inner queue");
    }
}

/// # `fork`-users notice.
///
/// Please not that this structure MUST NOT be used in a process differes from the one where it
/// was created!
pub struct Sender<T> {
    channel: Channel<T>,
}

unsafe impl<T: ShmemSafe> ShmemSafe for Sender<T> {}
unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Sync> Sync for Sender<T> {}

impl<T: ShmemSafe> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.channel.make_sender()
    }
}

fn atomic_checked_sub(variable: &AtomicUsize) -> usize {
    let mut counter = variable.load(Ordering::Relaxed);
    loop {
        let new_value = match counter.checked_sub(1) {
            Some(x) => x,
            None => {
                // This probably means the original `Channel` structure was destroyed. It's okay.
                return 0;
            }
        };
        match variable.compare_exchange_weak(
            counter,
            new_value,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(x) => {
                debug_assert_eq!(x, counter);
                // Yay, we did it!
                return new_value;
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
        let senders = &unsafe { self.channel.inner.as_ref() }.senders;
        if atomic_checked_sub(senders) == 0 {
            log::debug!("Last sender destroyed; sending item_maybe_available event");
            unsafe { self.channel.inner.as_ref() }
                .item_maybe_available
                .broadcast();
        }
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
        unsafe { self.channel.inner.as_ref() }.try_send_once(item)
    }

    /// Tries to send data until a timeout is reached.
    #[inline(always)]
    pub fn try_send_until(&self, item: T, until: Instant) -> Result<(), TrySendError<T>> {
        unsafe { self.channel.inner.as_ref() }.try_send_until(item, until)
    }

    /// When no receivers are available, returns `Err(item)`.
    #[inline(always)]
    pub fn send(&self, item: T) -> Result<(), T> {
        let inner = unsafe { self.channel.inner.as_ref() };
        inner.try_send(item)
    }

    /// Tries to send data until a timeout is reached.
    #[inline(always)]
    pub fn try_send_timeout(&self, item: T, timeout: Duration) -> Result<(), TrySendError<T>> {
        self.try_send_until(item, Instant::now() + timeout)
    }
}

/// # `fork`-users notice.
///
/// Please not that this structure MUST NOT be used in a process differes from the one where it
/// was created!
pub struct Receiver<T> {
    channel: Channel<T>,
}

unsafe impl<T: ShmemSafe> ShmemSafe for Receiver<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Sync> Sync for Receiver<T> {}

impl<T: ShmemSafe> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        self.channel.make_receiver()
    }
}

impl<T> Receiver<T> {
    /// Receives an item from the channel.
    ///
    /// If there are no senders left, `Err(())` is returned.
    #[inline(always)]
    pub fn receive(&self) -> Option<T> {
        let inner = unsafe { self.channel.inner.as_ref() };
        inner.receive()
    }

    /// Waits for a message to appear.
    pub fn wait_for_message_timeout(&self, timeout: Duration) -> Result<(), WaitingError> {
        unsafe { self.channel.inner.as_ref() }.wait_for_message_timeout(timeout)
    }

    /// Waits for a message to appear.
    pub fn wait_for_message_until(&self, until: Instant) -> Result<(), WaitingError> {
        unsafe { self.channel.inner.as_ref() }.wait_for_message_until(until)
    }

    /// Tries to receive an item once.
    #[inline(always)]
    pub fn try_receive(&self) -> Result<T, TryReceiveError> {
        unsafe { self.channel.inner.as_ref() }.try_receive_once()
    }

    /// Tries to receive an item once within a given timeout.
    #[inline(always)]
    pub fn try_receive_until(&self, until: Instant) -> Result<T, TryReceiveError> {
        unsafe { self.channel.inner.as_ref() }.try_receive_until(until)
    }

    /// Tries to receive an item once within a given timeout.
    #[inline(always)]
    pub fn try_receive_timeout(&self, timeout: Duration) -> Result<T, TryReceiveError> {
        unsafe { self.channel.inner.as_ref() }.try_receive_until(Instant::now() + timeout)
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let receivers = &unsafe { self.channel.inner.as_ref() }.receivers;
        if atomic_checked_sub(receivers) == 0 {
            log::debug!("Last receiver destroyed; sending space_maybe_available event");
            unsafe { self.channel.inner.as_ref() }
                .space_maybe_available
                .broadcast();
        }
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

#[derive(Debug, Snafu)]
pub enum WaitingError {
    QueueDropped,
    TimeoutReached,
}

impl<T> Inner<T> {
    #[inline(always)]
    fn push(
        &self,
        mut guard: MutexGuard<VecDeque<T, MemmapAlloc>>,
        item: T,
    ) -> Result<(), raw_vec::Error> {
        guard.push_back(item)?;
        drop(guard);
        log::debug!("Notifying that an item is available to be read");
        self.item_maybe_available.signal();
        log::debug!("Notifying {:p}", &self.item_maybe_available);

        Ok(())
    }

    #[inline(always)]
    fn pop(&self, guard: &mut MutexGuard<VecDeque<T, MemmapAlloc>>) -> Option<T> {
        let item = guard.pop_front()?;
        self.space_maybe_available.signal();
        Some(item)
    }

    /// Waits for a message to appear for a certain time.
    pub fn wait_for_message_timeout(&self, timeout: Duration) -> Result<(), WaitingError> {
        self.wait_for_message_until(Instant::now() + timeout)
    }

    /// Waits for a message to appear for a certain time.
    pub fn wait_for_message_until(&self, until: Instant) -> Result<(), WaitingError> {
        // Let's check that the main process haven't started destruction process.
        if self.dropping.load(Ordering::SeqCst) {
            return Err(WaitingError::QueueDropped);
        }
        let mut guard = match self.mutex.try_lock_until(until) {
            None => return Err(WaitingError::TimeoutReached),
            Some(guard) => guard,
        };
        // while guard.is_empty() {
        log::debug!("Locking on condvar {:p}", &self.item_maybe_available);
        if self
            .item_maybe_available
            .wait_until(&mut guard, until, |queue| {
                !queue.is_empty() || self.dropping.load(Ordering::SeqCst)
            })
            .is_err()
        {
            log::debug!("Condvar {:p} reported timeout", &self.item_maybe_available);
            return Err(WaitingError::TimeoutReached);
        } else {
            log::debug!("Condvar {:p} reported ready", &self.item_maybe_available);
        }

        Ok(())
    }

    /// Tries to receive an item [forerver, or until no senders are available].
    pub fn receive(&self) -> Option<T> {
        const LOCK_TIMEOUT: Duration = Duration::from_secs(1);
        let mut guard = loop {
            // Let's check that the main process haven't started destruction process.
            if self.dropping.load(Ordering::SeqCst) {
                return None;
            }
            match self.mutex.try_lock_for(LOCK_TIMEOUT) {
                None => {
                    log::warn!("Unable to lock a mutex for {:?}", LOCK_TIMEOUT);
                    continue;
                }
                Some(guard) => break guard,
            };
        };
        log::debug!("Lock obtained");
        self.item_maybe_available.wait(&mut guard, |queue| {
            !queue.is_empty()
                || self.senders.load(Ordering::SeqCst) == 0
                || self.dropping.load(Ordering::SeqCst)
        });
        self.pop(&mut guard)
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
        if self
            .item_maybe_available
            .wait_until(&mut guard, until, |queue| {
                !queue.is_empty()
                    || self.senders.load(Ordering::SeqCst) == 0
                    || self.dropping.load(Ordering::SeqCst)
            })
            .is_err()
        {
            Err(TryReceiveError::TimedOut)
        } else if self.senders.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst) {
            Err(TryReceiveError::NoSenders)
        } else {
            Ok(self.pop(&mut guard).expect("There should be something!!"))
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

    /// Tries to send an item [forever, or until there are no more receivers].
    pub fn try_send(&self, item: T) -> Result<(), T> {
        const LOCK_TIMEOUT: Duration = Duration::from_secs(1);
        let mut guard = loop {
            // Let's check that the main process haven't started destruction process.
            if self.dropping.load(Ordering::SeqCst) {
                return Err(item);
            }
            match self.mutex.try_lock_for(LOCK_TIMEOUT) {
                None => {
                    log::warn!("Unable to obtain a lock for {:?}", LOCK_TIMEOUT);
                    continue;
                }
                Some(guard) => break guard,
            };
        };
        log::debug!("Lock obtained");

        self.space_maybe_available.wait(&mut guard, |queue| {
            queue.remaining_capacity() != 0
                || self.receivers.load(Ordering::SeqCst) == 0
                || self.dropping.load(Ordering::SeqCst)
        });
        if self.receivers.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst) {
            return Err(item);
        }
        self.push(guard, item).expect("Shouldn't fail");
        Ok(())
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

        if self
            .space_maybe_available
            .wait_until(&mut guard, until, |queue| {
                queue.remaining_capacity() != 0
                    || self.receivers.load(Ordering::SeqCst) == 0
                    || self.dropping.load(Ordering::SeqCst)
            })
            .is_err()
        {
            return Err(TrySendError::TimedOut(item));
        }

        if self.receivers.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst) {
            return Err(TrySendError::NoReceivers(item));
        }
        self.push(guard, item).expect("Shouldn't fail");
        Ok(())
    }

    /// Tries to send an item without blocking.
    pub fn try_send_once(&self, item: T) -> Result<(), TrySendError<T>> {
        // Let's check that the main process haven't started destruction process.
        if self.dropping.load(Ordering::SeqCst) {
            return Err(TrySendError::NoReceivers(item));
        }
        let guard = match self.mutex.try_lock() {
            Some(guard) => guard,
            None => return Err(TrySendError::TimedOut(item)),
        };
        if guard.remaining_capacity() == 0 {
            Err(TrySendError::TimedOut(item))
        } else if self.receivers.load(Ordering::SeqCst) == 0 || self.dropping.load(Ordering::SeqCst)
        {
            Err(TrySendError::NoReceivers(item))
        } else {
            self.push(guard, item).expect("Shouldn't fail");
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use nix::unistd::{fork, ForkResult};
    use std::{io::Write, thread};

    fn init_logging() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Warn)
            .format(|f, record| {
                writeln!(
                    f,
                    "[{:5}] [{}] {}:{} {}",
                    record.level(),
                    record.module_path().unwrap(),
                    record.file().unwrap(),
                    record.line().unwrap(),
                    record.args()
                )
            })
            .is_test(true)
            .try_init();
    }

    #[test]
    fn check() {
        init_logging();

        let channel = Channel::<usize>::new(1234).unwrap();

        let processor = channel.make_receiver();

        match fork().unwrap() {
            ForkResult::Parent { child: _ } => {
                // Wait for a sender to initialize.
                thread::sleep(Duration::from_millis(100));

                log::info!("Waiting for request");
                let value = processor.receive().unwrap();
                log::info!("Received value");
                assert_eq!(value, 1234);
            }
            ForkResult::Child => {
                let sender = channel.make_sender();
                log::info!("Sending");
                sender.send(1234).unwrap();
                log::info!("Sent!")
            }
        }
    }
}
