use alloc_collections::{alloc::Error, Alloc};
use core::{alloc::Layout, mem::ManuallyDrop, ptr::NonNull};

pub struct Scoped<A: Alloc> {
    ptr: NonNull<u8>,
    layout: Layout,
    allocator: ManuallyDrop<A>,
}

impl<A: Alloc> Scoped<A> {
    pub fn new(mut allocator: A, layout: Layout) -> Result<Self, Error> {
        let ptr = unsafe { allocator.alloc(layout) }?;
        Ok(Scoped {
            ptr,
            layout,
            allocator: ManuallyDrop::new(allocator),
        })
    }

    pub fn as_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    pub fn into_raw(self) -> (NonNull<u8>, A) {
        let mut this = ManuallyDrop::new(self);
        let ptr = this.ptr;
        let allocator = unsafe { ManuallyDrop::take(&mut this.allocator) };
        (ptr, allocator)
    }
}

impl<A: Alloc> Drop for Scoped<A> {
    fn drop(&mut self) {
        unsafe {
            let mut allocator = ManuallyDrop::take(&mut self.allocator);
            allocator.dealloc(self.ptr, self.layout);
        }
    }
}
