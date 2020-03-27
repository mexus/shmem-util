use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use rand::{
    distributions::{Distribution, Standard},
    rngs::SmallRng,
    Rng, SeedableRng,
};
use shmem_utils::{
    channel::{Channel, Receiver, Sender},
    shmem_safe::ShmemSafe,
};
use std::{thread, time::Duration};

macro_rules! declare_data_type {
    ($name:ident, $size:expr) => {
        #[derive(Clone)]
        #[repr(transparent)]
        struct $name([u8; $size]);

        impl Distribution<$name> for Standard {
            fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> $name {
                let mut array = [0; $size];
                rng.fill(&mut array);
                $name(array)
            }
        }

        unsafe impl ShmemSafe for $name {}
    };
}

declare_data_type!(SmolData, 16);
declare_data_type!(MediumData, 128);
declare_data_type!(BigData, 1024);

fn ping_pong_server<T: ShmemSafe>(
    requests: Receiver<Request<T>>,
    responds: Sender<Response<T>>,
) -> thread::JoinHandle<()>
where
    T: Send + 'static,
{
    thread::spawn(move || {
        while let Some(Request(item)) = requests.receive() {
            if responds.send(Response(item)).is_err() {
                break;
            }
        }
    })
}

struct Request<T>(T);
unsafe impl<T: ShmemSafe> ShmemSafe for Request<T> {}

struct Response<T>(T);
unsafe impl<T: ShmemSafe> ShmemSafe for Response<T> {}

fn bench<T>(b: &mut criterion::Bencher<'_>, queue_length: usize)
where
    T: ShmemSafe + Send + 'static,
    Standard: Distribution<T>,
{
    let mut rng = SmallRng::seed_from_u64(1234);

    let chan1 = Channel::<Request<T>>::new(queue_length).unwrap();
    let chan2 = Channel::<Response<T>>::new(queue_length).unwrap();

    let from_client = chan1.make_receiver();
    let to_server = chan1.make_sender();

    let from_server = chan2.make_receiver();
    let to_client = chan2.make_sender();

    let servers: Vec<_> = (0..1)
        .map(move |_| {
            let from_client = from_client.clone();
            let to_client = to_client.clone();
            ping_pong_server(from_client, to_client)
        })
        .collect();

    b.iter_batched(
        || rng.gen(),
        move |item| {
            assert!(to_server.send(Request(item)).is_ok());
            assert!(from_server.receive().is_some());
        },
        BatchSize::SmallInput,
    );
    servers.into_iter().for_each(|handle| {
        let _ = handle.join();
    });
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("shmem queue");

    const CAPACITY: usize = 2;

    group.bench_function(format!("smol ping, cap = {}", CAPACITY), |b| {
        bench::<SmolData>(b, CAPACITY)
    });
    group.bench_function(format!("medium ping, cap = {}", CAPACITY), |b| {
        bench::<MediumData>(b, CAPACITY)
    });
    group.bench_function(format!("big ping, cap = {}", CAPACITY), |b| {
        bench::<BigData>(b, CAPACITY)
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets = criterion_benchmark
}
criterion_main!(benches);
