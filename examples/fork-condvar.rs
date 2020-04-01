use alloc_collections::boxes::CustomBox;
use nix::{
    sys::wait::waitpid,
    unistd::{fork, ForkResult},
};
use parking_lot::{Condvar, Mutex};
use shmem_utils::allocator::ShmemAlloc;
use std::{thread, time::Duration};

fn main() {
    let allocator = ShmemAlloc::new(1024).unwrap();
    let condvar = CustomBox::new_in((Condvar::new(), Mutex::new(())), allocator).unwrap();
    match fork().unwrap() {
        ForkResult::Parent { child } => {
            thread::sleep(Duration::from_secs(1));
            {
                let guard = condvar.1.lock();
                condvar.0.notify_one();
                drop(guard);
            }
            thread::sleep(Duration::from_secs(1));
            waitpid(child, None).unwrap();
        }
        ForkResult::Child => {
            let mut guard = condvar.1.lock();
            eprintln!("Waiting...");
            condvar.0.wait(&mut guard);
            eprintln!("Waiting done!");
        }
    }
}
