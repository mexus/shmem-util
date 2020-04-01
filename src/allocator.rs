use crate::{memmap::MemmapAlloc, mutex::Mutex, ShmemSafe};
use alloc::Excess;
use alloc_collections::{alloc, boxes::CustomBox, Alloc};
use chunks::Chunks;
use snafu::{ensure, ResultExt, Snafu};
use std::{alloc::Layout, ptr::NonNull};

mod chunks;
mod free_chunk;

/// Shared memory allocator with a fixed capacity.
// A note on `derive`: it is safe to copy pointers since we never drop the data and access it only
// in immutable ways.
#[derive(Clone, Copy)]
pub struct ShmemAlloc {
    free_chunks: NonNull<Mutex<Chunks>>,
    memory_buffer: NonNull<u8>,
}

unsafe impl ShmemSafe for ShmemAlloc {}
unsafe impl Send for ShmemAlloc {}
unsafe impl Sync for ShmemAlloc {}

/// Shared memory allocator initialization error.
#[derive(Debug, Snafu)]
pub enum InitializationError {
    /// Zero bytes capacity is not supported.
    #[snafu(display("Zero bytes capacity is not supported"))]
    ZeroCapacity,

    /// Unaligned capacity.
    #[snafu(display(
        "Unaligned capacity: {}, alignment: {}",
        capacity,
        ShmemAlloc::ALIGNMENT
    ))]
    UnalignedCapacity { capacity: usize },

    /// Unable to initialize internal vector with {} chunks.
    #[snafu(display(
        "Unable to initialize internal vector with {} chunks: {}",
        chunks,
        source
    ))]
    InternalVector {
        chunks: usize,
        source: alloc_collections::raw_vec::Error,
    },

    /// Unable to initialize internal buffer.
    #[snafu(display("Unable to initialize internal buffer: {}", source))]
    InternalBuffer { source: alloc::Error },

    /// Unable to initialize a memory buffer.
    #[snafu(display("Unable to initialize a memory buffer: {}", source))]
    MemoryBuffer { source: alloc::Error },
}

impl ShmemAlloc {
    /// Alignment of the allocated memory chunks.
    pub const ALIGNMENT: usize = 8;

    /// Initializes a new allocator with a given capacity (in bytes).
    ///
    /// # Notice
    ///
    /// Please not that the internal structures of the allocator will only be freed by the
    /// operating system when the application terminates.
    ///
    /// On the other hand, if this method fails, all the memory is freed.
    ///
    /// Will fail if either:
    /// * `capacity` is zero,
    /// * `capacity` is not divisible by `ShmemAlloc::ALIGNMENT`,
    /// * allocation of internal buffers in shared memory fails.
    pub fn new(capacity: usize) -> Result<Self, InitializationError> {
        ensure!(capacity != 0, ZeroCapacity);
        ensure!(
            capacity % Self::ALIGNMENT == 0,
            UnalignedCapacity { capacity }
        );

        let max_chunks_count = capacity / Self::ALIGNMENT;
        let free_chunks = Chunks::new(max_chunks_count).context(InternalVector {
            chunks: max_chunks_count,
        })?;
        let free_chunks =
            CustomBox::new_in(Mutex::new(free_chunks), MemmapAlloc).context(InternalBuffer)?;
        let (free_chunks, _allocator) = free_chunks.into_raw_parts();

        let memory_layout =
            Layout::from_size_align(capacity, Self::ALIGNMENT).expect("Should be well-formed");
        let memory_buffer = unsafe { MemmapAlloc.alloc(memory_layout) }.context(MemoryBuffer)?;

        log::trace!(
            "Shared memory allocator ready (capacity = {} bytes, alignment = {})",
            capacity,
            Self::ALIGNMENT
        );

        Ok(ShmemAlloc {
            free_chunks,
            memory_buffer,
        })
    }

    #[inline]
    fn check_alignment(&self, layout: &Layout) -> Result<(), alloc::Error> {
        if layout.align() == 0 || layout.size() == 0 {
            return Ok(());
        }
        if Self::ALIGNMENT % layout.align() != 0 {
            log::error!("Unsupported alignment {}", layout.align());
            alloc::AllocationError { layout: *layout }.fail()
        } else {
            log::trace!("Layout {:?} ok", layout);
            Ok(())
        }
    }
}

unsafe impl alloc::Alloc for ShmemAlloc {
    #[inline]
    unsafe fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, alloc::Error> {
        self.check_alignment(&layout)?;
        let Excess(ptr, _) = self.alloc_excess(layout)?;
        Ok(ptr)
    }

    #[inline]
    unsafe fn alloc_zeroed(&mut self, layout: Layout) -> Result<NonNull<u8>, alloc::Error> {
        // Since `mmap` returns zeroed memory, we don't need to do anything else.
        self.alloc(layout)
    }

    #[inline]
    fn usable_size(&self, layout: &Layout) -> (usize, usize) {
        if self.check_alignment(layout).is_ok() {
            let size = layout.size();
            let size = if size % Chunks::CHUNK_SIZE == 0 {
                size
            } else {
                (size / Chunks::CHUNK_SIZE) * (Chunks::CHUNK_SIZE + 1)
            };
            (size, size)
        } else {
            (0, 0)
        }
    }

    unsafe fn alloc_excess(&mut self, layout: Layout) -> Result<Excess, alloc::Error> {
        self.check_alignment(&layout)?;
        if layout.size() == 0 {
            return Ok(Excess(NonNull::new_unchecked(layout.align() as *mut u8), 0));
        }
        let mut free_chunks = self.free_chunks.as_ref().lock();
        if let Some(chunk) = free_chunks.get(layout.size()) {
            let usable_size = chunk.count() * Chunks::CHUNK_SIZE;
            let ptr = chunk.begin().into_absolute(self.memory_buffer);
            // log::debug!("Created pointer {:p}", ptr);
            Ok(Excess(ptr, usable_size))
        } else {
            log::error!(
                "No free space available to allocate {} bytes",
                layout.size()
            );
            alloc::AllocationError { layout }.fail()
        }
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if layout.align() == ptr.as_ptr() as usize {
            return;
        }
        if let Err(e) = self.check_alignment(&layout) {
            log::error!("Deallocation failed because of invalid alignment: {}", e);
            return;
        }
        let shift = match (ptr.as_ptr() as usize).checked_sub(self.memory_buffer.as_ptr() as usize)
        {
            None => {
                log::error!("Provided pointer {:p} is out of range", ptr);
                return;
            }
            Some(shift) => shift,
        };
        let mut free_chunks = self.free_chunks.as_ref().lock();
        free_chunks.return_back(shift, layout.size());
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc_collections::Vec;
    use quickcheck_macros::quickcheck;
    use std::{cmp, fmt::Debug, mem::size_of};

    #[test]
    fn check() {
        crate::init_logging();

        let mut allocator = ShmemAlloc::new(1024).unwrap();
        let mut value_ptr = allocator.alloc_one::<u128>().unwrap();

        unsafe {
            value_ptr.as_ptr().write(442343);
        }
        let value = unsafe { value_ptr.as_mut() };
        assert_eq!(*value, 442343);
        *value = 1;
        assert_eq!(*value, 1);
    }

    fn populate_vector<T>(original: std::vec::Vec<T>)
    where
        T: Clone + PartialEq + Debug,
    {
        crate::init_logging();

        let capacity = size_of::<T>() * original.capacity() * 4 / ShmemAlloc::ALIGNMENT
            * ShmemAlloc::ALIGNMENT
            + ShmemAlloc::ALIGNMENT;
        let capacity = cmp::max(ShmemAlloc::ALIGNMENT, capacity);
        let allocator = ShmemAlloc::new(capacity).unwrap();
        let mut v = Vec::<T, _>::new_in(allocator);
        for item in &original {
            v.push(item.clone()).unwrap();
        }

        assert_eq!(&v[..], &original[..])
    }

    #[quickcheck]
    fn populate_vector_usize(original: std::vec::Vec<usize>) {
        populate_vector(original)
    }

    #[quickcheck]
    fn populate_vector_isize(original: std::vec::Vec<isize>) {
        populate_vector(original)
    }

    #[quickcheck]
    fn populate_vector_u8(original: std::vec::Vec<u8>) {
        populate_vector(original)
    }
}
