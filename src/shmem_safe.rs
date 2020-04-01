use alloc_collections::{boxes::CustomBox, deque::VecDeque, Alloc, IndexMap, Vec};
use core::sync::atomic;

/// Marker trait for types that are safe to be transmitted between processes.
pub unsafe trait ShmemSafe {}

// Unsigned types:
unsafe impl ShmemSafe for u8 {}
unsafe impl ShmemSafe for atomic::AtomicU8 {}

unsafe impl ShmemSafe for u16 {}
unsafe impl ShmemSafe for atomic::AtomicU16 {}

unsafe impl ShmemSafe for u32 {}
unsafe impl ShmemSafe for atomic::AtomicU32 {}

unsafe impl ShmemSafe for u64 {}
unsafe impl ShmemSafe for atomic::AtomicU64 {}

unsafe impl ShmemSafe for u128 {}
// No AtomicU128

unsafe impl ShmemSafe for usize {}
unsafe impl ShmemSafe for atomic::AtomicUsize {}

// Signed types:
unsafe impl ShmemSafe for i8 {}
unsafe impl ShmemSafe for atomic::AtomicI8 {}

unsafe impl ShmemSafe for i16 {}
unsafe impl ShmemSafe for atomic::AtomicI16 {}

unsafe impl ShmemSafe for i32 {}
unsafe impl ShmemSafe for atomic::AtomicI32 {}

unsafe impl ShmemSafe for i64 {}
unsafe impl ShmemSafe for atomic::AtomicI64 {}

unsafe impl ShmemSafe for i128 {}
// No AtomicI128

unsafe impl ShmemSafe for isize {}
unsafe impl ShmemSafe for atomic::AtomicIsize {}

// Elementary types.
unsafe impl ShmemSafe for char {}
unsafe impl ShmemSafe for bool {}
unsafe impl ShmemSafe for atomic::AtomicBool {}

// Pointers.
unsafe impl<T: ShmemSafe> ShmemSafe for *mut T {}
unsafe impl<T: ShmemSafe> ShmemSafe for *const T {}
unsafe impl<T: ShmemSafe> ShmemSafe for atomic::AtomicPtr<T> {}

// Some "heap"-allocated types.
unsafe impl<T: ShmemSafe, A: Alloc + ShmemSafe> ShmemSafe for VecDeque<T, A> {}
unsafe impl<T: ShmemSafe, A: Alloc + ShmemSafe> ShmemSafe for Vec<T, A> {}
unsafe impl<T: ShmemSafe, A: Alloc + ShmemSafe> ShmemSafe for IndexMap<T, A> {}
unsafe impl<T: ShmemSafe, A: Alloc + ShmemSafe> ShmemSafe for CustomBox<T, A> {}

macro_rules! impl_array {
    ($($size:expr),* $(,)?) => {
        $(
            unsafe impl<T: ShmemSafe> ShmemSafe for [T; $size] {}
        )*
    };
}

macro_rules! impl_tuple {
    (  $( { $($ty:ident),+ $(,)? } ),* $(,)?  ) => {
        $(
            unsafe impl< $( $ty: ShmemSafe, )* > ShmemSafe for ( $($ty),* ) {}
        )*
    };
}

impl_array!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32
);

impl_tuple!(
    {T1, T2},
    {T1, T2, T3},
    {T1, T2, T3, T4},
    {T1, T2, T3, T4, T5},
    {T1, T2, T3, T4, T5, T6},
    {T1, T2, T3, T4, T5, T6, T7},
    {T1, T2, T3, T4, T5, T6, T7, T8},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15},
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26, T27},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26, T27, T28},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26, T27, T28, T29},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26, T27, T28, T29, T30},

    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26, T27, T28, T29, T30, T31},
    
    {T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16,
    T17, T18, T19, T20, T21, T22, T23, T24, T25, T26, T27, T28, T29, T30, T31, T32},
);
