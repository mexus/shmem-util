use alloc_collections::boxes::CustomBox;
use nix::unistd::{fork, ForkResult};
use parking_lot::{Condvar, Mutex};
use shmem_utils::memmap::MemmapAlloc;
use std::{thread, time::Duration};

fn main() {
    let condvar = CustomBox::new_in((Condvar::new(), Mutex::new(())), MemmapAlloc).unwrap();
    match fork().unwrap() {
        ForkResult::Parent { .. } => {
            thread::sleep(Duration::from_secs(1));
            {
                let guard = condvar.1.lock();
                condvar.0.notify_one();
                drop(guard);
            }
            thread::sleep(Duration::from_secs(1));
        }
        ForkResult::Child => {
            let mut guard = condvar.1.lock();
            eprintln!("Waiting...");
            condvar.0.wait(&mut guard);
            eprintln!("Waiting done!");
        }
    }
}
