//! `mmap` naïve allocator.

use alloc_collections::{
    alloc::{AllocationError, Error},
    Alloc,
};
use core::{
    alloc::Layout,
    ptr::{null_mut, NonNull},
};
use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};

/// Naïve `mmap` allocator.
#[derive(Debug, Clone, Copy)]
pub struct MemmapAlloc;

unsafe impl Alloc for MemmapAlloc {
    unsafe fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, Error> {
        match mmap(
            null_mut(),
            layout.size(),
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_ANONYMOUS | MapFlags::MAP_SHARED,
            -1,
            0,
        )
        .map(NonNull::new)
        {
            Ok(Some(raw_ptr)) => {
                if raw_ptr.as_ptr() as usize % layout.align() != 0 {
                    log::error!(
                        "Mmap returned unsuitably aligned memory at {:p}, requested layout {:?}",
                        raw_ptr,
                        layout
                    );
                    AllocationError { layout }.fail()
                } else {
                    log::trace!(
                        "Mapped memory region at {:p} with layout {:?}",
                        raw_ptr,
                        layout
                    );
                    Ok(raw_ptr.cast())
                }
            }
            Ok(None) => {
                log::error!("mmap returned null pointer");
                AllocationError { layout }.fail()
            }
            Err(e) => {
                log::error!("mmap error: {}", e);
                AllocationError { layout }.fail()
            }
        }
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if let Err(e) = munmap(ptr.as_ptr().cast(), layout.size()) {
            log::error!("munmap at {:p}, layout {:?} failed: {}", ptr, layout, e);
        } else {
            log::trace!(
                "Unmapped memory region at {:p} with layout {:?}",
                ptr,
                layout
            );
        }
    }
}
