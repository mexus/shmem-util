use nix::{
    sys::wait::waitpid,
    unistd::{fork, ForkResult, Pid},
};
use shmem_utils::{allocator::ShmemAlloc, channel::Channel};
use std::{io::Write, thread, time::Duration};

fn main() {
    env_logger::builder()
        .format(|f, record| {
            writeln!(
                f,
                "[{}] [{}] [{:5}] [{}] [{}:{}] {}",
                Pid::this(),
                chrono::Utc::now().format("%T%.f"),
                record.level(),
                record.module_path().unwrap(),
                record.file().unwrap(),
                record.line().unwrap(),
                record.args()
            )
        })
        .init();

    let allocator = ShmemAlloc::new(10 * 1024 * 1024).unwrap();

    let channel = Channel::<i32>::new(1234, allocator).unwrap();

    let receiver = channel.make_receiver();

    match fork().unwrap() {
        ForkResult::Parent { child } => {
            log::info!("Waiting for the first message to arrive");
            let _ = receiver
                .wait_for_message_timeout(Duration::from_secs(1))
                .unwrap();
            let x = receiver.receive().expect("Unable to receive a message");
            assert_eq!(x, child.as_raw());
            log::info!("Everything's okay on the parent's side");

            thread::sleep(Duration::from_secs(1));
            waitpid(child, None).unwrap();
        }
        ForkResult::Child => {
            let sender = channel.make_sender();
            sender
                .send(Pid::this().as_raw())
                .expect("Unable to send a message");
            log::info!("Message sent from the child");
            thread::sleep(Duration::from_secs(1));
        }
    }
}
