use anmixiu_reactive::{OwnerRegistry, Signal};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn signal_notify(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("signal_notify_and_take_dirty");
    for owner_count in [1_usize, 100, 1_000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(owner_count),
            &owner_count,
            |bencher, &owner_count| {
                bencher.iter_batched(
                    || {
                        let owners = OwnerRegistry::new();
                        let signal = Signal::new(0_u64);
                        for _ in 0..owner_count {
                            let owner = owners.create_owner();
                            let _ = owners.observe(owner, || signal.get());
                        }
                        (owners, signal)
                    },
                    |(owners, signal)| {
                        signal.set(1);
                        black_box(owners.take_dirty());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn dirty_deduplication(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("dirty_deduplication");
    for invalidation_count in [10_usize, 1_000, 100_000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(invalidation_count),
            &invalidation_count,
            |bencher, &invalidation_count| {
                let owners = OwnerRegistry::new();
                let owner = owners.create_owner();
                bencher.iter(|| {
                    for _ in 0..invalidation_count {
                        black_box(owners.mark_dirty(owner));
                    }
                    assert_eq!(owners.take_dirty(), vec![owner]);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, signal_notify, dirty_deduplication);
criterion_main!(benches);
