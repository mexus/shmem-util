use crate::{allocator::ShmemAlloc, memmap::MemmapAlloc};
use alloc_collections::{deque::VecDeque, IndexMap, Vec};

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
