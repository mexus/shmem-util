use crate::{autoclean, memmap::MemmapAlloc};
use alloc_collections::{
    alloc::{self, AllocationError},
    boxes::CustomBox,
    deque::VecDeque,
    Alloc,
};
use core::{alloc::Layout, ptr::NonNull};
use parking_lot::Mutex;
use snafu::{ensure, OptionExt, ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Unable to get page size: {}", source))]
    PageSize { source: crate::pagesize::Error },

    #[snafu(display(
        "Provided type has alignment {:?}, which is incompatible with page size {}",
        layout,
        pagesize
    ))]
    UnsupportedAlignment { layout: Layout, pagesize: usize },

    #[snafu(display("mmap allocation failure: {}", source))]
    MmapError {
        source: alloc_collections::alloc::Error,
    },

    #[snafu(display("Internal queue initialization failure: {}", source))]
    InternalQueue {
        source: alloc_collections::raw_vec::Error,
    },

    #[snafu(display("Internal queue initialization failure: {}", source))]
    InternalQueueBoxing {
        source: alloc_collections::alloc::Error,
    },

    #[snafu(display("Unable to put a mutex into the memory"))]
    MutexInit,

    #[snafu(display("Requested capacity is too large"))]
    CapacityTooLarge,
}

/// Arena-like shared-memory allocator.
#[derive(Debug, Clone)]
pub struct ShmemAlloc {
    data_blocks: NonNull<u8>,
    data_blocks_count: usize,
    block_layout: Layout,

    free_blocks: NonNull<Mutex<VecDeque<NonNull<u8>, MemmapAlloc>>>,
}

unsafe impl Send for ShmemAlloc {}
unsafe impl Sync for ShmemAlloc {}

macro_rules! free_blocks {
    ($this:ident) => {
        unsafe { $this.free_blocks.as_mut() }.lock()
    };
}

impl ShmemAlloc {
    /// Initializes memory.
    pub fn new<T>(capacity: usize) -> Result<Self, Error> {
        Self::with_layout(Layout::new::<T>(), capacity)
    }

    /// Initializes memory.
    pub fn with_layout(layout: Layout, capacity: usize) -> Result<Self, Error> {
        let pagesize = crate::pagesize::get_pagesize().context(PageSize)?;

        ensure!(
            pagesize % layout.align() == 0,
            UnsupportedAlignment { layout, pagesize }
        );

        let length = capacity
            .checked_mul(layout.size())
            .context(CapacityTooLarge)?;

        let extra_space = length % pagesize;
        let extra_capacity = extra_space / layout.size();
        let capacity = capacity + extra_capacity;

        let total_layout = unsafe { Layout::from_size_align_unchecked(length, layout.align()) };
        let mut user_data = autoclean::Scoped::new(MemmapAlloc, total_layout).context(MmapError)?;

        let mut free_blocks = VecDeque::<NonNull<u8>, _>::with_capacity_in(capacity, MemmapAlloc)
            .context(InternalQueue)?;
        for idx in 0..capacity {
            unsafe {
                let ptr = user_data.as_ptr().add(idx * layout.size());
                let ptr = NonNull::new_unchecked(ptr);
                free_blocks.push_back(ptr).context(InternalQueue)?;
            }
        }
        let free_blocks = Mutex::new(free_blocks);
        let free_blocks =
            CustomBox::new_in(free_blocks, MemmapAlloc).context(InternalQueueBoxing)?;
        let (free_blocks, ..) = free_blocks.into_raw_parts();

        let (user_data, _) = user_data.into_raw();

        Ok(ShmemAlloc {
            data_blocks: user_data,
            data_blocks_count: capacity,
            free_blocks: free_blocks.cast(),
            block_layout: layout,
        })
    }

    fn take_free(&mut self) -> Option<NonNull<u8>> {
        let mut free_blocks = free_blocks!(self);
        let free_ptr = free_blocks.pop_front()?;
        Some(free_ptr)
    }

    fn return_back(&mut self, ptr: NonNull<u8>) {
        let mut free_blocks = free_blocks!(self);
        assert!(
            ptr.as_ptr() as usize >= self.data_blocks.as_ptr() as usize,
            "{:p} < {:p}",
            ptr.as_ptr(),
            self.data_blocks.as_ptr()
        );
        free_blocks.push_back(ptr).expect("Shouldn't happen");
    }
}

impl Drop for ShmemAlloc {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::from_size_align_unchecked(
                self.block_layout.size() * self.data_blocks_count,
                self.block_layout.align(),
            );
            MemmapAlloc.dealloc(self.data_blocks.cast(), layout)
        }
    }
}

unsafe impl Alloc for ShmemAlloc {
    unsafe fn alloc(
        &mut self,
        layout: Layout,
    ) -> Result<NonNull<u8>, alloc_collections::alloc::Error> {
        ensure!(
            layout.align() == self.block_layout.align()
                && layout.size() % self.block_layout.size() == 0,
            AllocationError { layout }
        );
        self.take_free().map(|p| p.cast()).ok_or_else(|| {
            log::error!("No free chunks available");
            alloc::Error::AllocationError { layout }
        })
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, _layout: Layout) {
        self.return_back(ptr.cast())
    }
}

#[cfg(test)]
mod test {
    // use super::*;
    // use core::time::Duration;
    // use nix::unistd::{fork, ForkResult};
    // use std::thread;

    // #[test]
    // fn check_fork() {
    //     type Vec = alloc_collections::Vec<usize, ShmemAlloc>;

    //     let data_allocator = ShmemAlloc::new::<usize>(1024).unwrap();
    //     let vec_allocator = ShmemAlloc::new::<Mutex<Vec>>(2).unwrap();

    //     let vector1 = Mutex::new(Vec::new_in(data_allocator.clone()));
    //     let vector2 = Mutex::new(Vec::new_in(data_allocator));

    //     let vector1 = CustomBox::new_in(vector1, vec_allocator.clone()).unwrap();
    //     let vector2 = CustomBox::new_in(vector2, vec_allocator).unwrap();

    //     match fork().unwrap() {
    //         ForkResult::Parent { .. } => {
    //             vector1.lock().push(1413).unwrap();
    //             vector2.lock().push(2594).unwrap();
    //             thread::sleep(Duration::from_millis(200));

    //             assert_eq!(vector1.lock().pop(), Some(1413 + 1));
    //             assert_eq!(vector2.lock().pop(), Some(2594 + 1));
    //         }
    //         ForkResult::Child => {
    //             thread::sleep(Duration::from_millis(100));
    //             let first = vector1.lock().pop().unwrap();
    //             let second = vector2.lock().pop().unwrap();

    //             vector1.lock().push(first + 1).unwrap();
    //             vector2.lock().push(second + 1).unwrap();
    //         }
    //     }
    // }
}
