use criterion::{black_box, criterion_group, criterion_main, Criterion};
use chimera_operator::worker::pool::WorkerPool;

fn bench_worker_pool_creation(c: &mut Criterion) {
    c.bench_function("worker_pool_creation", |b| {
        b.iter(|| {
            WorkerPool::new(black_box(4))
        })
    });
}

criterion_group!(benches, bench_worker_pool_creation);
criterion_main!(benches);