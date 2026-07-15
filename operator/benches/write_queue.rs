use criterion::{black_box, criterion_group, criterion_main, Criterion};
use chimera_operator::queue::write::WriteQueue;

fn bench_write_queue_creation(c: &mut Criterion) {
    c.bench_function("write_queue_creation", |b| {
        b.iter(|| {
            WriteQueue::new(black_box(100))
        })
    });
}

criterion_group!(benches, bench_write_queue_creation);
criterion_main!(benches);