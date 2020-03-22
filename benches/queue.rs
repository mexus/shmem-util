use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use rand::Rng;
use shmem::fixed_queue::FixedQueue;
use std::{
    iter::repeat_with,
    mem,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    thread,
    time::Duration,
};

const SIZE: usize = 16;
#[derive(Clone)]
struct BigData([u8; SIZE]);

fn generate() -> Vec<BigData> {
    let mut rng = rand::thread_rng();
    (0..10_240)
        .map(|_| {
            let mut array = [0; SIZE];
            rng.fill(&mut array);
            BigData(array)
        })
        .collect()
}

fn looped(data: &[BigData]) -> impl Iterator<Item = BigData> + '_ {
    let mut cnt = 0;
    let len = data.len();
    repeat_with(move || {
        cnt = (cnt + 1) % len;
        data[cnt].clone()
    })
}

fn ping_pong_server<T>(
    requests: &FixedQueue<T>,
    responds: &FixedQueue<T>,
    running: &Arc<AtomicBool>,
) -> thread::JoinHandle<()>
where
    T: Send + 'static,
{
    let requests = requests.clone();
    let responds = responds.clone();
    let running = Arc::clone(&running);
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            while let Some(item) = requests.try_pop_front_timeout(Duration::from_secs(1)) {
                responds.push_back(item);
            }
        }
    })
}

fn bench_length(b: &mut criterion::Bencher<'_>, data: &[BigData], queue_length: usize) {
    let mut iter = looped(data);
    let requests = FixedQueue::<BigData>::new(queue_length).unwrap();
    let responds = FixedQueue::<BigData>::new(queue_length).unwrap();

    let running = Arc::new(AtomicBool::new(true));

    let servers: Vec<_> = (0..1)
        .map(|_| ping_pong_server(&requests, &responds, &running))
        .collect();

    b.iter(|| {
        requests.push_back(iter.next().unwrap());
        responds.pop_back()
    });
    running.store(false, Ordering::Relaxed);
    servers.into_iter().for_each(|handle| {
        let _ = handle.join();
    });
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("shmem queue");
    group.throughput(Throughput::Bytes(mem::size_of::<BigData>() as u64));
    for &capacity in &[1, 10, 100, 1000] {
        let data = generate();
        group.bench_function(format!("ping, cap = {}", capacity), |b| {
            bench_length(b, &data, capacity)
        });
    }
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
