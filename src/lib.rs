pub mod allocator;
mod autoclean;
pub mod channel;
pub mod condvar;
pub mod memmap;
pub mod mutex;
pub mod pagesize;

mod time;

mod shmem_safe;
pub use shmem_safe::ShmemSafe;

fn strerror(errno: libc::c_int) -> String {
    unsafe {
        let msg = libc::strerror(errno);
        std::ffi::CStr::from_ptr(msg).to_string_lossy().to_string()
    }
}

#[cfg(test)]
fn init_logging() {
    use nix::unistd::{gettid, Pid};
    use std::io::Write;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .format(|f, record| {
            writeln!(
                f,
                "[{:5}] [{}:{}] [{}] [{}] [{}:{}] {}",
                record.level(),
                Pid::this(),
                gettid(),
                chrono::Utc::now().format("%T%.6f"),
                record.module_path().unwrap_or_default(),
                record.file().unwrap_or_default(),
                record.line().unwrap_or_default(),
                record.args()
            )
        })
        .is_test(true)
        .try_init();
}
