use criterion::{Criterion, criterion_group, criterion_main};

fn bench_ner_cpu_single_window(c: &mut Criterion) {
    c.bench_function("ner_cpu_single_window", |b| {
        b.iter(|| {
            let _tokens = "The quarterly revenue report shows $5.2M in ARR across all regions."
                .split_whitespace()
                .count();
        });
    });
}

fn bench_ner_cpu_multi_window(c: &mut Criterion) {
    c.bench_function("ner_cpu_multi_window", |b| {
        b.iter(|| {
            let windows: Vec<String> = (0..10)
                .map(|i| format!("Window {i}: Revenue milestone {i} completed successfully."))
                .collect();
            let _total_tokens: usize = windows.iter().map(|w| w.split_whitespace().count()).sum();
        });
    });
}

criterion_group!(
    benches,
    bench_ner_cpu_single_window,
    bench_ner_cpu_multi_window
);
criterion_main!(benches);
