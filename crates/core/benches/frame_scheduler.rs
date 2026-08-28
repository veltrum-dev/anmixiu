use anmixiu_core::{FrameBatcher, WindowId};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn scheduler_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("frame_scheduler");
    for count in [32_u64, 4_096] {
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |bencher, count| {
                bencher.iter(|| {
                    let window = WindowId::new(1);
                    let mut batcher = FrameBatcher::new(8);
                    for component in 0..*count {
                        batcher.mark_dirty(window, component, None);
                        batcher.mark_dirty(window, component, None);
                    }
                    let dirty = batcher.begin_frame(window);
                    let _ = batcher.finish_frame(window, !dirty.is_empty());
                    dirty.len()
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, scheduler_benchmarks);
criterion_main!(benches);
