use criterion::{Criterion, criterion_group, criterion_main};

fn bench_ner_metal_single_window(c: &mut Criterion) {
    c.bench_function("ner_metal_single_window", |b| {
        b.iter(|| {
            let _tokens = "The quarterly revenue report shows $5.2M in ARR across all regions."
                .split_whitespace()
                .count();
        });
    });
}

criterion_group!(benches, bench_ner_metal_single_window);
criterion_main!(benches);
