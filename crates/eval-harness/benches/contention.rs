use criterion::{Criterion, criterion_group, criterion_main};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn bench_contention_single_client(c: &mut Criterion) {
    c.bench_function("contention_single_client", |b| {
        let counter = Arc::new(AtomicUsize::new(0));
        b.iter(|| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
    });
}

fn bench_contention_multi_client(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");
    for num_clients in [2, 4, 8] {
        group.bench_function(format!("clients_{num_clients}"), |b| {
            b.iter(|| {
                let counter = Arc::new(AtomicUsize::new(0));
                let handles: Vec<_> = (0..num_clients)
                    .map(|_| {
                        let c = counter.clone();
                        std::thread::spawn(move || {
                            for _ in 0..100 {
                                c.fetch_add(1, Ordering::Relaxed);
                            }
                        })
                    })
                    .collect();
                for h in handles {
                    h.join().unwrap();
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_contention_single_client,
    bench_contention_multi_client
);
criterion_main!(benches);
