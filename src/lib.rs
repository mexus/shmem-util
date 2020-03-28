pub mod allocator;
mod autoclean;
pub mod channel;
pub mod memmap;
pub mod pagesize;

mod shmem_safe;
pub use shmem_safe::ShmemSafe;